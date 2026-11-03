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

#[derive(Clone)]
pub(crate) struct Artifact {
    pub source: PathBuf,
    pub loaded: PathBuf,
    pub sha256: String,
}

pub(crate) fn private_dir(path: &Path) -> Result<(), PluginError> {
    fs::create_dir_all(path).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io)?;
    }
    Ok(())
}

fn extension() -> &'static str {
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
    private_dir(&directory)?;
    let loaded = directory.join(format!("{}-{}.{}", sha256, Uuid::new_v4(), extension()));
    write_new(&loaded, &bytes)?;
    Ok(Artifact {
        source,
        loaded,
        sha256,
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
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)?;
    Ok(())
}

pub(crate) fn package(
    root: &Path,
    artifact: &Artifact,
    manifest: &Manifest,
) -> Result<PathBuf, PluginError> {
    validate_manifest(manifest)?;
    let directory = root
        .join("packages")
        .join(&manifest.id)
        .join(&manifest.version)
        .join(&artifact.sha256);
    private_dir(&directory)?;
    let path = directory.join(format!("plugin.{}", extension()));
    let bytes = fs::read(&artifact.loaded).map_err(io)?;
    if format!("{:x}", Sha256::digest(&bytes)) != artifact.sha256 {
        return Err(error(PLUGIN_IO, "staged plugin hash changed"));
    }
    if path.exists() {
        if fs::read(&path).map_err(io)? != bytes {
            return Err(error(PLUGIN_IO, "immutable package was modified"));
        }
    } else {
        write_new(&path, &bytes)?;
    }
    Ok(path)
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
        return Ok(pair);
    }
    let library = ManuallyDrop::new(unsafe { Library::new(&artifact.loaded) }.map_err(io)?);
    metrics.loaded_libraries.fetch_add(1, Ordering::Relaxed);
    let entry = unsafe { library.get::<SeEntryFn>(SE_ENTRY_SYMBOL) }
        .map_err(|_| error(PLUGIN_ABI_UNSUPPORTED, "missing se_plugin_entry_v1"))?;
    let mut api = std::ptr::null();
    status(unsafe { entry(SE_ABI_VERSION, SE_ABI_VERSION, &mut api) })?;
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
    Ok((api, parse_manifest(&bytes)?))
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

#[cfg(test)]
mod tests {
    use super::*;
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
