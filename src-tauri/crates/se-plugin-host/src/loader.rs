use crate::validation::{error, io, status};
use libloading::Library;
use se_plugin_abi::*;
use se_plugin_protocol::*;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) struct Artifact {
    pub source: PathBuf,
    pub loaded: PathBuf,
    pub sha256: String,
    pub archive: Option<Arc<crate::archive::Archive>>,
}

pub(crate) fn extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

pub(crate) fn stage(root: &Path, boot: &str, source: &Path) -> Result<Artifact, PluginError> {
    if !source.is_absolute() {
        return Err(error(
            PLUGIN_INVALID_REQUEST,
            "plugin path must be absolute",
        ));
    }
    if crate::archive::is_archive(source)? {
        return crate::archive::stage(root, boot, source);
    }
    stage_library(root, boot, source)
}

pub(crate) fn stage_library(
    root: &Path,
    boot: &str,
    source: &Path,
) -> Result<Artifact, PluginError> {
    let source = source.canonicalize().map_err(io)?;
    let mut file = fs::File::open(&source).map_err(io)?;
    let metadata = file.metadata().map_err(io)?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > 64 * 1024 * 1024
        || source.extension().and_then(|s| s.to_str()) != Some(extension())
    {
        return Err(error(
            PLUGIN_INVALID_REQUEST,
            "expected a local native library of at most 64 MiB",
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 != metadata.len() || bytes.len() > 64 * 1024 * 1024 {
        return Err(error(PLUGIN_IO, "plugin file changed while being copied"));
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let directory = root.join("loaded").join(boot);
    crate::archive::private_directory(root, &directory)?;
    let loaded = directory.join(format!("{}-{}.{}", sha256, Uuid::new_v4(), extension()));
    write_new(&loaded, &bytes)?;
    Ok(Artifact {
        source,
        loaded,
        sha256,
        archive: None,
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), PluginError> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(io)?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result.map_err(io)
}

/// `None` means absent; never follow a package symlink, even a dangling one.
fn package_matches(path: &Path, bytes: &[u8]) -> Result<Option<bool>, PluginError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(fs::read(path).map_err(io)? == bytes)),
        Ok(_) => Err(error(PLUGIN_IO, "immutable package is not a regular file")),
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(failure) => Err(io(failure)),
    }
}

fn publish_package(path: &Path, bytes: &[u8]) -> Result<(), PluginError> {
    // Multiple host instances may share a package root. Serialize validation and
    // repair so a second publisher never renames over an already repaired file.
    static PUBLICATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _publication = PUBLICATION
        .lock()
        .map_err(|_| error(PLUGIN_IO, "package publication lock poisoned"))?;
    let temporary = path.with_file_name(format!(".plugin-{}.tmp", Uuid::new_v4()));
    write_new(&temporary, bytes)?;
    let result = (|| loop {
        match package_matches(path, bytes)? {
            Some(true) => return Ok(()),
            // A previous interrupted write may have left a partial final file.
            // Replace only mismatching regular files, with fully synced bytes.
            Some(false) => return fs::rename(&temporary, path).map_err(io),
            // A hard link publishes the synced inode atomically and, unlike
            // rename, cannot clobber a complete file published in the meantime.
            None => match fs::hard_link(&temporary, path) {
                Ok(()) => return Ok(()),
                // Another publisher won: validate it rather than overwrite it.
                Err(failure) if failure.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(failure) => return Err(io(failure)),
            },
        }
    })();
    // After rename this name is already gone; after linking (or any failure)
    // remove our private temporary, never another publisher's file.
    let _ = fs::remove_file(&temporary);
    result
}

pub(crate) fn package(
    root: &Path,
    artifact: &Artifact,
    manifest: &Manifest,
) -> Result<PathBuf, PluginError> {
    validate_candidate(artifact, manifest)?;
    if let Some(archive) = &artifact.archive {
        return archive.publish(root);
    }
    let directory = root
        .join("packages")
        .join(&manifest.id)
        .join(&manifest.version)
        .join(&artifact.sha256);
    crate::archive::private_directory(root, &directory)?;
    let path = directory.join(format!("plugin.{}", extension()));
    let bytes = fs::read(&artifact.loaded).map_err(io)?;
    if format!("{:x}", Sha256::digest(&bytes)) != artifact.sha256 {
        return Err(error(PLUGIN_IO, "staged plugin hash changed"));
    }
    if package_matches(&path, &bytes)? != Some(true) {
        publish_package(&path, &bytes)?;
    }
    Ok(path)
}

pub(crate) fn validate_candidate(
    artifact: &Artifact,
    manifest: &Manifest,
) -> Result<(), PluginError> {
    validate_manifest(manifest)?;
    match &artifact.archive {
        Some(archive) if &archive.manifest != manifest => Err(error(
            PLUGIN_INVALID_MANIFEST,
            "archive manifest differs from native ABI manifest",
        )),
        None if manifest
            .contributes
            .views
            .iter()
            .any(|view| view.kind == Some(ViewKind::Web)) =>
        {
            Err(error(
                PLUGIN_INVALID_PACKAGE,
                "web views require an asset-bearing .seplugin package",
            ))
        }
        _ => Ok(()),
    }
}

/// Runs exclusively on an ABI worker. The mapping is intentionally never unloaded,
/// including symbol/manifest failures: native code may have retained global state.
pub(crate) fn load(
    artifact: &Artifact,
    metrics: &Arc<crate::abi_worker::Metrics>,
) -> Result<(SePluginApiV1, Manifest), PluginError> {
    #[cfg(test)]
    if let Some(pair) = fixtures().lock().unwrap().get(&artifact.sha256).cloned() {
        metrics.loaded_libraries.fetch_add(1, Ordering::Relaxed);
        validate_candidate(artifact, &pair.1)?;
        return Ok(pair);
    }
    let library = ManuallyDrop::new(unsafe { Library::new(&artifact.loaded) }.map_err(io)?);
    metrics.loaded_libraries.fetch_add(1, Ordering::Relaxed);
    let entry = unsafe { library.get::<SeEntryFn>(SEP_ENTRY_SYMBOL) }
        .map_err(|_| error(PLUGIN_ABI_UNSUPPORTED, "missing se_plugin_entry_v1"))?;
    let mut api = std::ptr::null();
    status(unsafe { entry(SEP_ABI_VERSION, SEP_ABI_VERSION, &mut api) })?;
    let api = *unsafe { validate_plugin_api(api) }.map_err(|failure| {
        error(
            match failure {
                AbiValidationError::StructTooSmall { .. } => PLUGIN_STRUCT_TOO_SMALL,
                _ => PLUGIN_ABI_UNSUPPORTED,
            },
            "invalid plugin ABI v1 table",
        )
    })?;
    let mut bytes = vec![0; MAX_JSON_BYTES];
    let mut output = SeOutput {
        ptr: bytes.as_mut_ptr(),
        capacity: bytes.len(),
        len: 0,
    };
    status(unsafe { api.manifest.expect("validated manifest")(&mut output) })?;
    if output.ptr != bytes.as_mut_ptr()
        || output.capacity != bytes.len()
        || output.len > bytes.len()
    {
        return Err(error(
            PLUGIN_INVALID_MANIFEST,
            "invalid manifest output buffer",
        ));
    }
    bytes.truncate(output.len);
    let manifest = parse_manifest(&bytes)?;
    validate_candidate(artifact, &manifest)?;
    Ok((api, manifest))
}

#[cfg(test)]
fn fixtures(
) -> &'static std::sync::Mutex<std::collections::BTreeMap<String, (SePluginApiV1, Manifest)>> {
    static FIXTURES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<String, (SePluginApiV1, Manifest)>>,
    > = std::sync::OnceLock::new();
    FIXTURES.get_or_init(Default::default)
}
#[cfg(test)]
pub(crate) fn register_fixture(source: &Path, manifest: Manifest, api: SePluginApiV1) {
    let bytes = Uuid::new_v4().to_string().into_bytes();
    fs::write(source, &bytes).unwrap();
    fixtures()
        .lock()
        .unwrap()
        .insert(format!("{:x}", Sha256::digest(bytes)), (api, manifest));
}

pub(crate) fn hash(path: &Path) -> Result<String, PluginError> {
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(io)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let n = file.read(&mut bytes).map_err(io)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package_fixture(root: &Path) -> (Artifact, Manifest, PathBuf) {
        let source = root.join(format!("source.{}", extension()));
        fs::write(&source, b"complete native plugin bytes").unwrap();
        let artifact = stage(root, "boot", &source).unwrap();
        let manifest = Manifest {
            schema_version: 1,
            id: "test.loader".to_owned(),
            name: "Loader".to_owned(),
            version: "1.0.0".to_owned(),
            abi: AbiRange { min: 1, max: 1 },
            contributes: Contributions {
                commands: vec![],
                views: vec![],
                configuration: ConfigurationSchema {
                    title: "Settings".to_owned(),
                    properties: Default::default(),
                },
                event_subscriptions: vec![],
                menus: vec![],
                keybindings: vec![],
                status_items: vec![],
                processes: vec![],
            },
            services: Services {
                provides: vec![],
                requires: vec![],
            },
        };
        let directory = root
            .join("packages")
            .join(&manifest.id)
            .join(&manifest.version)
            .join(&artifact.sha256);
        fs::create_dir_all(&directory).unwrap();
        crate::registry::owner_only_directory(&directory).unwrap();
        let path = directory.join(format!("plugin.{}", extension()));
        (artifact, manifest, path)
    }

    fn assert_only_package(path: &Path) {
        let entries: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(entries, vec![path.to_path_buf()]);
    }

    #[test]
    fn partial_final_package_is_repaired_atomically_and_retry_succeeds() {
        let root = tempfile::tempdir().unwrap();
        let (artifact, manifest, path) = package_fixture(root.path());
        fs::write(&path, b"partial").unwrap();
        // An existing reader keeps the old inode, proving repair is replacement,
        // not truncation and rewriting of the final pathname.
        let mut old_file = fs::File::open(&path).unwrap();
        assert_eq!(package(root.path(), &artifact, &manifest).unwrap(), path);
        assert_eq!(
            fs::read(&path).unwrap(),
            fs::read(&artifact.loaded).unwrap()
        );
        let mut old_bytes = Vec::new();
        old_file.read_to_end(&mut old_bytes).unwrap();
        assert_eq!(old_bytes, b"partial");
        assert_eq!(package(root.path(), &artifact, &manifest).unwrap(), path);
        assert_only_package(&path);
    }

    #[test]
    fn package_publication_is_private_and_reuses_complete_files() {
        let root = tempfile::tempdir().unwrap();
        let (artifact, manifest, path) = package_fixture(root.path());
        package(root.path(), &artifact, &manifest).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            fs::read(&artifact.loaded).unwrap()
        );
        let before = fs::metadata(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(before.permissions().mode() & 0o777, 0o600);
        }
        package(root.path(), &artifact, &manifest).unwrap();
        // Exercise the publisher's no-overwrite check as well as package's fast path.
        publish_package(&path, &fs::read(&artifact.loaded).unwrap()).unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(before.ino(), after.ino());
        }
        assert_only_package(&path);
    }

    #[test]
    fn failed_publication_removes_temporary_and_refuses_nonregular_final() {
        let root = tempfile::tempdir().unwrap();
        let (artifact, manifest, path) = package_fixture(root.path());
        fs::create_dir(&path).unwrap();
        assert_eq!(
            package(root.path(), &artifact, &manifest).unwrap_err().code,
            PLUGIN_IO
        );
        // Call the publication helper directly to force failure after temp sync.
        assert_eq!(
            publish_package(&path, b"complete").unwrap_err().code,
            PLUGIN_IO
        );
        assert!(path.is_dir());
        assert_only_package(&path);
    }

    #[cfg(unix)]
    #[test]
    fn package_refuses_existing_and_dangling_symlinks() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let (artifact, manifest, path) = package_fixture(root.path());
        let missing = root.path().join("missing");
        for target in [&artifact.loaded, &missing] {
            symlink(target, &path).unwrap();
            assert_eq!(
                package(root.path(), &artifact, &manifest).unwrap_err().code,
                PLUGIN_IO
            );
            assert_eq!(
                publish_package(&path, b"complete").unwrap_err().code,
                PLUGIN_IO
            );
            assert_eq!(fs::read_link(&path).unwrap(), *target);
            assert_only_package(&path);
            fs::remove_file(&path).unwrap();
        }
        assert!(!missing.exists());
    }

    #[test]
    fn write_new_collision_preserves_the_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("existing.tmp");
        fs::write(&path, b"another writer").unwrap();
        assert!(write_new(&path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"another writer");
    }

    #[test]
    fn rejects_relative_and_non_library_paths() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            stage(root.path(), "boot", Path::new("relative.so"))
                .err()
                .unwrap()
                .code,
            PLUGIN_INVALID_REQUEST
        );
        let path = root.path().join("not.txt");
        fs::write(&path, b"not a library").unwrap();
        assert!(stage(root.path(), "boot", &path).is_err());
    }
}
