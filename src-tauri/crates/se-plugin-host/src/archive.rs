//! Verified `.seplugin` staging. No executable bytes are loaded until every
//! inventory member is verified; publication renames a complete directory.
use crate::{loader, registry};
use se_plugin_protocol::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

type Result<T> = std::result::Result<T, PluginError>;

#[derive(Debug)]
pub(crate) struct Archive {
    pub dir: PathBuf,
    /// Digest of the original ZIP bytes, not the selected native artifact.
    pub sha256: String,
    pub descriptor: PackageDescriptor,
    pub manifest: Manifest,
    pub native: String,
    remove_on_drop: bool,
    cleanup_parent: Option<PathBuf>,
}
impl Drop for Archive {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_dir_all(self.cleanup_parent.as_ref().unwrap_or(&self.dir));
        }
    }
}
impl Archive {
    pub(crate) fn metadata(&self, root: &Path) -> registry::PackageMetadata {
        registry::PackageMetadata {
            digest: self.sha256.clone(),
            root: self.published_root(root),
            target: target().to_owned(),
            native_path: self.native.clone(),
        }
    }
    fn published_root(&self, root: &Path) -> PathBuf {
        root.join("packages")
            .join(&self.manifest.id)
            .join(&self.manifest.version)
            .join(&self.sha256)
    }
    pub(crate) fn publish(&self, root: &Path) -> Result<PathBuf> {
        // Store owns the cross-process profile lock. Serialize concurrent install
        // requests too, so equal digests are verified/reused, never replaced.
        static PUBLICATION: Mutex<()> = Mutex::new(());
        let _guard = PUBLICATION
            .lock()
            .map_err(|_| invalid("publication lock poisoned"))?;
        let root = root.canonicalize().map_err(registry::io_error)?;
        let destination = self.published_root(&root);
        private_directory(&root, destination.parent().unwrap())?;
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                verify_directory(&destination, &self.descriptor, &self.manifest)?;
                return Ok(destination.join(&self.native));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(registry::io_error(error)),
        }
        verify_directory(&self.dir, &self.descriptor, &self.manifest)?;
        sync_tree(&self.dir)?;
        fs::rename(&self.dir, &destination).map_err(registry::io_error)?;
        registry::sync_directory(destination.parent().unwrap())?;
        registry::sync_directory(self.dir.parent().unwrap())?;
        Ok(destination.join(&self.native))
    }
}
struct Cleanup(Option<PathBuf>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_dir_all(path);
        }
    }
}

pub(crate) fn is_archive(path: &Path) -> Result<bool> {
    if path
        .extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| x.eq_ignore_ascii_case("seplugin"))
    {
        return Ok(true);
    }
    let mut input = File::open(path).map_err(registry::io_error)?;
    let mut magic = [0; 4];
    match input.read_exact(&mut magic) {
        Ok(()) => Ok(matches!(
            &magic,
            b"PK\x03\x04" | b"PK\x05\x06" | b"PK\x07\x08"
        )),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(registry::io_error(error)),
    }
}
pub(crate) fn stage(root: &Path, boot: &str, source: &Path) -> Result<loader::Artifact> {
    let source = source.canonicalize().map_err(registry::io_error)?;
    let root = root.canonicalize().map_err(registry::io_error)?;
    let dir = root
        .join("staging")
        .join(boot)
        .join(format!("archive-{}", uuid::Uuid::new_v4()));
    private_directory(&root, &dir)?;
    let mut cleanup = Cleanup(Some(dir.clone()));
    // Snapshot once, hashing exactly the bytes we later parse. Reopening the
    // original after hashing would allow an in-place source mutation race.
    let snapshot = dir.join(".source.zip");
    let mut input = File::open(&source).map_err(registry::io_error)?;
    if !input.metadata().map_err(registry::io_error)?.is_file() {
        return Err(invalid("archive source must be a regular file"));
    }
    let mut output = new_file(&snapshot)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buffer).map_err(registry::io_error)?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_PACKAGE_ARCHIVE_BYTES {
            return Err(invalid("archive exceeds compressed size limit"));
        }
        output.write_all(&buffer[..n]).map_err(registry::io_error)?;
        digest.update(&buffer[..n]);
    }
    output.sync_all().map_err(registry::io_error)?;
    drop(output);
    let mut zip = zip::ZipArchive::new(File::open(&snapshot).map_err(registry::io_error)?)
        .map_err(|_| invalid("invalid ZIP archive"))?;
    if zip.len() > MAX_PACKAGE_FILES + 1 {
        return Err(invalid("too many archive files"));
    }
    let mut names = BTreeSet::new();
    let mut nodes = BTreeMap::new();
    let mut indexes = BTreeMap::new();
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|_| invalid("invalid ZIP member"))?;
        let name = entry.name().to_owned();
        // No directory records, aliases, symlinks, special files or alternate
        // roots. Prefix aliases matter on case-insensitive filesystems as well.
        if !is_package_path(&name)
            || entry.is_dir()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
            || !names.insert(name.to_ascii_lowercase())
            || !(name == "package.json"
                || name == "manifest.json"
                || name.starts_with("native/")
                || name.starts_with("ui/"))
        {
            return Err(invalid("unsafe or duplicate archive path"));
        }
        record_path(&mut nodes, &name)?;
        indexes.insert(name, index);
    }
    let package_bytes = zip_bytes(
        &mut zip,
        *indexes
            .get("package.json")
            .ok_or_else(|| invalid("missing package.json"))?,
        MAX_PACKAGE_DESCRIPTOR_BYTES as u64,
    )?;
    let descriptor = parse_package_descriptor(&package_bytes)?;
    let native = descriptor
        .native
        .get(target())
        .ok_or_else(|| {
            PluginError::new(
                PLUGIN_NOT_SUPPORTED,
                "archive has no native artifact for this target",
            )
        })?
        .clone();
    let expected: BTreeSet<_> = descriptor
        .files
        .iter()
        .map(|f| f.path.as_str())
        .chain(std::iter::once("package.json"))
        .collect();
    if indexes.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err(invalid("archive inventory mismatch"));
    }
    let package_dir = dir.join("package");
    private_directory(&root, &package_dir)?;
    for member in &descriptor.files {
        let path = package_dir.join(&member.path);
        private_directory(&root, path.parent().unwrap())?;
        let mut input = zip
            .by_index(indexes[&member.path])
            .map_err(|_| invalid("invalid ZIP member"))?;
        if input.size() != member.size {
            return Err(invalid("declared package size mismatch"));
        }
        let mut output = new_file(&path)?;
        #[cfg(unix)]
        if input.unix_mode().is_some_and(|mode| mode & 0o111 != 0) {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(registry::io_error)?;
        }
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        loop {
            let n = input
                .read(&mut buffer)
                .map_err(|_| invalid("invalid ZIP payload"))?;
            if n == 0 {
                break;
            }
            bytes = bytes.saturating_add(n as u64);
            if bytes > member.size {
                return Err(invalid("expanded package size mismatch"));
            }
            hash.update(&buffer[..n]);
            output.write_all(&buffer[..n]).map_err(registry::io_error)?;
        }
        if bytes != member.size || format!("{:x}", hash.finalize()) != member.sha256 {
            return Err(invalid("package file hash mismatch"));
        }
        output.sync_all().map_err(registry::io_error)?;
    }
    let mut package_output = new_file(&package_dir.join("package.json"))?;
    package_output
        .write_all(&package_bytes)
        .map_err(registry::io_error)?;
    package_output.sync_all().map_err(registry::io_error)?;
    let manifest = parse_manifest(&read_small(&package_dir.join("manifest.json"))?)?;
    validate_web_entries(&descriptor, &manifest)?;
    let archive = Arc::new(Archive {
        dir: package_dir,
        sha256: format!("{:x}", digest.finalize()),
        descriptor,
        manifest,
        native,
        remove_on_drop: true,
        cleanup_parent: Some(dir.clone()),
    });
    // loader::stage only copies verified native bytes; it does not dlopen.
    let mut artifact = loader::stage_library(&root, boot, &archive.dir.join(&archive.native))?;
    verify_native_copy(&artifact, &archive.descriptor, &archive.native)?;
    artifact.source = source;
    artifact.archive = Some(archive);
    drop(zip);
    fs::remove_file(snapshot).map_err(registry::io_error)?;
    cleanup.0 = None;
    Ok(artifact)
}

pub(crate) fn stage_published(
    root: &Path,
    boot: &str,
    source: &Path,
    metadata: &registry::PackageMetadata,
) -> Result<loader::Artifact> {
    let root = root.canonicalize().map_err(registry::io_error)?;
    if !is_sha256(&metadata.digest) || metadata.target != target() {
        return Err(invalid("published package digest or target mismatch"));
    }
    checked_directory(&root, &metadata.root)?;
    let descriptor = parse_package_descriptor(&read_small(&metadata.root.join("package.json"))?)?;
    let manifest = parse_manifest(&read_small(&metadata.root.join("manifest.json"))?)?;
    let native = descriptor
        .native
        .get(target())
        .ok_or_else(|| invalid("published native target missing"))?
        .clone();
    let expected_root = root
        .join("packages")
        .join(&manifest.id)
        .join(&manifest.version)
        .join(&metadata.digest);
    if metadata.root != expected_root
        || native != metadata.native_path
        || source != metadata.root.join(&native)
    {
        return Err(invalid("published package identity mismatch"));
    }
    verify_directory(&metadata.root, &descriptor, &manifest)?;
    let mut artifact = loader::stage_library(&root, boot, source)?;
    verify_native_copy(&artifact, &descriptor, &native)?;
    artifact.archive = Some(Arc::new(Archive {
        dir: metadata.root.clone(),
        sha256: metadata.digest.clone(),
        descriptor,
        manifest,
        native,
        remove_on_drop: false,
        cleanup_parent: None,
    }));
    Ok(artifact)
}

fn verify_native_copy(
    artifact: &loader::Artifact,
    descriptor: &PackageDescriptor,
    native: &str,
) -> Result<()> {
    if !descriptor
        .files
        .iter()
        .any(|file| file.path == native && file.sha256 == artifact.sha256)
    {
        return Err(invalid("selected native changed while staging"));
    }
    Ok(())
}

pub(crate) fn target() -> &'static str {
    env!("SE_PLUGIN_TARGET")
}
fn invalid(message: &str) -> PluginError {
    PluginError::new(PLUGIN_INVALID_PACKAGE, message)
}

fn record_path(nodes: &mut BTreeMap<String, (String, bool)>, name: &str) -> Result<()> {
    let parts: Vec<_> = name.split('/').collect();
    for end in 1..=parts.len() {
        let path = parts[..end].join("/");
        let file = end == parts.len();
        match nodes.get(&path.to_ascii_lowercase()) {
            Some((old, was_file)) if old != &path || *was_file != file => {
                return Err(invalid("case or file/directory alias in package"))
            }
            _ => {
                nodes.insert(path.to_ascii_lowercase(), (path, file));
            }
        }
    }
    Ok(())
}
fn new_file(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(registry::io_error)?;
    registry::owner_only_file(path)?;
    Ok(file)
}
fn zip_bytes(zip: &mut zip::ZipArchive<File>, index: usize, limit: u64) -> Result<Vec<u8>> {
    let mut entry = zip
        .by_index(index)
        .map_err(|_| invalid("invalid ZIP member"))?;
    if entry.size() > limit {
        return Err(invalid("descriptor too large"));
    }
    let mut bytes = Vec::new();
    (&mut entry)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("invalid ZIP descriptor"))?;
    if bytes.len() as u64 > limit {
        return Err(invalid("descriptor too large"));
    }
    Ok(bytes)
}
fn read_small(path: &Path) -> Result<Vec<u8>> {
    if !fs::symlink_metadata(path)
        .map_err(registry::io_error)?
        .is_file()
    {
        return Err(invalid("descriptor is not a regular file"));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(registry::io_error)?
        .take(MAX_PACKAGE_DESCRIPTOR_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(registry::io_error)?;
    if bytes.len() > MAX_PACKAGE_DESCRIPTOR_BYTES {
        return Err(invalid("descriptor too large"));
    }
    Ok(bytes)
}
fn validate_web_entries(descriptor: &PackageDescriptor, manifest: &Manifest) -> Result<()> {
    for view in &manifest.contributes.views {
        if view.kind == Some(ViewKind::Web)
            && !view.entry.as_ref().is_some_and(|entry| {
                descriptor
                    .files
                    .iter()
                    .any(|file| file.path == format!("ui/{entry}"))
            })
        {
            return Err(invalid("web entry missing from package inventory"));
        }
    }
    Ok(())
}

/// Walk every component before creating/chmodding it. `create_dir_all` would
/// follow an existing symlink and could create/chmod paths outside the profile.
pub(crate) fn private_directory(root: &Path, path: &Path) -> Result<()> {
    walk_directory(root, path, true)
}
fn checked_directory(root: &Path, path: &Path) -> Result<()> {
    walk_directory(root, path, false)
}
fn walk_directory(root: &Path, path: &Path, create: bool) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid("directory escapes plugin root"))?;
    let mut current = root.to_path_buf();
    if !fs::symlink_metadata(&current)
        .map_err(registry::io_error)?
        .is_dir()
    {
        return Err(invalid("plugin root is not a directory"));
    }
    for part in relative.components() {
        let Component::Normal(part) = part else {
            return Err(invalid("non-normal package directory"));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(invalid("package directory is a symlink or non-directory")),
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        if !fs::symlink_metadata(&current)
                            .map_err(registry::io_error)?
                            .is_dir()
                        {
                            return Err(invalid("package directory changed during creation"));
                        }
                    }
                    Err(error) => return Err(registry::io_error(error)),
                }
                registry::sync_directory(current.parent().unwrap())?;
            }
            Err(error) => return Err(registry::io_error(error)),
        }
        if create {
            registry::owner_only_directory(&current)?;
        }
    }
    Ok(())
}
fn verify_directory(dir: &Path, descriptor: &PackageDescriptor, manifest: &Manifest) -> Result<()> {
    if !fs::symlink_metadata(dir)
        .map_err(registry::io_error)?
        .is_dir()
    {
        return Err(invalid("package root is a symlink or non-directory"));
    }
    let expected: BTreeSet<_> = descriptor
        .files
        .iter()
        .map(|file| file.path.clone())
        .chain(std::iter::once("package.json".into()))
        .collect();
    let directories: BTreeSet<_> = expected
        .iter()
        .flat_map(|file| {
            file.match_indices('/')
                .map(|(index, _)| file[..index].to_owned())
        })
        .collect();
    let mut files = BTreeSet::new();
    collect_files(dir, dir, &directories, &mut files)?;
    if files != expected
        || parse_package_descriptor(&read_small(&dir.join("package.json"))?)? != *descriptor
        || parse_manifest(&read_small(&dir.join("manifest.json"))?)? != *manifest
    {
        return Err(invalid("published package inventory mismatch"));
    }
    for member in &descriptor.files {
        let path = dir.join(&member.path);
        let metadata = fs::symlink_metadata(&path).map_err(registry::io_error)?;
        if !metadata.is_file()
            || metadata.len() != member.size
            || loader::hash(&path)? != member.sha256
        {
            return Err(invalid("published package payload mismatch"));
        }
    }
    validate_web_entries(descriptor, manifest)
}
fn collect_files(
    root: &Path,
    dir: &Path,
    directories: &BTreeSet<String>,
    files: &mut BTreeSet<String>,
) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(registry::io_error)? {
        let entry = entry.map_err(registry::io_error)?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_str()
            .ok_or_else(|| invalid("non-ASCII package path"))?
            .to_owned();
        if !is_package_path(&relative) {
            return Err(invalid("unsafe published package path"));
        }
        let meta = fs::symlink_metadata(&path).map_err(registry::io_error)?;
        if meta.is_dir() {
            if !directories.contains(&relative) {
                return Err(invalid("unlisted published package directory"));
            }
            collect_files(root, &path, directories, files)?;
        } else if meta.is_file() {
            files.insert(relative);
            if files.len() > MAX_PACKAGE_FILES + 1 {
                return Err(invalid("too many published files"));
            }
        } else {
            return Err(invalid(
                "published package contains a symlink or special file",
            ));
        }
    }
    Ok(())
}
fn sync_tree(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(registry::io_error)? {
        let path = entry.map_err(registry::io_error)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(registry::io_error)?;
        if metadata.is_dir() {
            sync_tree(&path)?;
        } else if metadata.is_file() {
            File::open(&path)
                .map_err(registry::io_error)?
                .sync_all()
                .map_err(registry::io_error)?;
        } else {
            return Err(invalid("package changed during publication"));
        }
    }
    registry::sync_directory(dir)
}

pub(crate) fn package_root(native: &Path) -> Option<PathBuf> {
    native
        .ancestors()
        .skip(1)
        .take(8)
        .find(|root| root.join("package.json").is_file())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use zip::write::SimpleFileOptions;

    fn manifest(web: bool) -> Manifest {
        let views = if web {
            json!([{"id":"main","title":"Archive","location":"panel","kind":"web","entry":"index.html"}])
        } else {
            json!([])
        };
        parse_manifest(
            &serde_json::to_vec(&json!({
                "schemaVersion":1,"id":"archive.test","name":"Archive","version":"1.0.0",
                "abi":{"min":1,"max":1},"contributes":{"commands":[],"views":views,"configuration":{"title":"Settings","properties":{}}},
                "services":{"provides":[],"requires":[]}
            }))
            .unwrap(),
        )
        .unwrap()
    }
    fn fixture(
        base: &Path,
        mutate: impl FnOnce(&mut Value, &mut Vec<(String, Vec<u8>)>),
    ) -> PathBuf {
        let native = format!("native/{}/plugin.{}", target(), loader::extension());
        let mut files = vec![
            (
                "manifest.json".into(),
                serde_json::to_vec(&manifest(true)).unwrap(),
            ),
            (native.clone(), b"verified archive native bytes".to_vec()),
            (
                "ui/index.html".into(),
                b"<!doctype html><title>Archive</title>".to_vec(),
            ),
        ];
        let mut descriptor = json!({
            "schemaVersion":1,"ui":"ui","native":{(target()):native},
            "files":files.iter().map(|(path, bytes)| json!({
                "path":path,"size":bytes.len(),"sha256":format!("{:x}", Sha256::digest(bytes))
            })).collect::<Vec<_>>()
        });
        mutate(&mut descriptor, &mut files);
        let path = base.join("fixture.seplugin");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("package.json", options).unwrap();
        zip.write_all(&serde_json::to_vec(&descriptor).unwrap())
            .unwrap();
        for (name, bytes) in files {
            let mode = if name.starts_with("native/") {
                0o755
            } else {
                0o644
            };
            zip.start_file(name, options.unix_permissions(mode))
                .unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }
    fn host_root(base: &Path) -> PathBuf {
        let root = base.join("host");
        fs::create_dir(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn archive_preserves_owner_executable_for_managed_processes() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let source = fixture(base.path(), |_, _| {});
        let root = host_root(base.path());
        let artifact = stage(&root, "boot", &source).unwrap();
        let archive = artifact.archive.as_ref().unwrap();
        assert_eq!(
            fs::metadata(archive.dir.join(&archive.native))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(archive.dir.join("ui/index.html"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn archive_install_dispatches_by_extension_or_zip_magic() {
        let base = tempfile::tempdir().unwrap();
        let original = fixture(base.path(), |_, _| {});
        let renamed = base.path().join("package.bin");
        fs::rename(&original, &renamed).unwrap();
        assert!(is_archive(&renamed).unwrap());
        let root = host_root(base.path());
        let artifact = loader::stage(&root, "boot", &renamed).unwrap();
        assert!(artifact.archive.is_some());
        let invalid_extension = base.path().join("invalid.SEPLUGIN");
        fs::write(&invalid_extension, b"not zip").unwrap();
        assert!(is_archive(&invalid_extension).unwrap());
        assert!(loader::stage(&root, "boot", &invalid_extension).is_err());
        let bare = base
            .path()
            .join(format!("bare.{}", loader::extension()));
        fs::write(&bare, b"native").unwrap();
        assert!(!is_archive(&bare).unwrap());
        assert!(loader::stage(&root, "boot", &bare)
            .unwrap()
            .archive
            .is_none());
    }
    #[test]
    fn archive_rejects_escape_and_alias_paths() {
        for name in [
            "../escape",
            "/absolute",
            "ui/../escape",
            "ui//empty",
            "ui/a\\b",
            "ui/INDEX.html",
            "ui/%2e%2e/escape",
            "ui/__se__/host.js",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = host_root(temp.path());
            let path = fixture(temp.path(), |_, files| files.push((name.into(), vec![])));
            assert!(stage(&root, "boot", &path).is_err(), "accepted {name}");
            assert!(!root.join("loaded").exists());
            assert!(!temp.path().join("escape").exists());
        }
        for paths in [
            ["ui/Assets/a.js", "ui/assets/b.js"],
            ["ui/file", "ui/file/child"],
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = host_root(temp.path());
            let path = fixture(temp.path(), |_, files| {
                files.extend(paths.into_iter().map(|path| (path.into(), vec![])));
            });
            assert!(stage(&root, "boot", &path).is_err());
            assert!(!root.join("loaded").exists());
        }
    }

    #[test]
    fn archive_hash_failure_never_loads_native() {
        for member in ["manifest.json", "ui/index.html", "native"] {
            let temp = tempfile::tempdir().unwrap();
            let root = host_root(temp.path());
            let path = fixture(temp.path(), |_, files| {
                let (_, bytes) = files
                    .iter_mut()
                    .find(|(name, _)| {
                        name == member || (member == "native" && name.starts_with("native/"))
                    })
                    .unwrap();
                bytes[0] ^= 1; // same length, wrong digest
            });
            assert_eq!(
                stage(&root, "boot", &path).unwrap_err().code,
                PLUGIN_INVALID_PACKAGE
            );
            assert!(
                !root.join("loaded").exists(),
                "native copy must not even be staged"
            );
            assert!(!root.join("packages").exists());
            assert_eq!(fs::read_dir(root.join("staging/boot")).unwrap().count(), 0);
        }
    }

    #[test]
    fn archive_manifest_must_match_abi() {
        let temp = tempfile::tempdir().unwrap();
        let root = host_root(temp.path());
        let path = fixture(temp.path(), |_, _| {});
        let artifact = stage(&root, "boot", &path).unwrap();
        let archive = artifact.archive.as_ref().unwrap();
        let mut abi_manifest = archive.manifest.clone();
        abi_manifest.name = "different ABI manifest".into();
        assert_eq!(
            loader::validate_candidate(&artifact, &abi_manifest)
                .unwrap_err()
                .code,
            PLUGIN_INVALID_MANIFEST
        );
        assert!(loader::package(&root, &artifact, &abi_manifest).is_err());
        assert!(!archive.metadata(&root).root.exists());
        loader::validate_candidate(&artifact, &archive.manifest).unwrap();
        let bare_source = temp.path().join(format!("bare.{}", loader::extension()));
        fs::write(&bare_source, b"bare fixture native bytes").unwrap();
        let bare = loader::stage(&root, "boot", &bare_source).unwrap();
        assert_eq!(
            loader::validate_candidate(&bare, &archive.manifest)
                .unwrap_err()
                .code,
            PLUGIN_INVALID_PACKAGE
        );
        assert!(loader::package(&root, &bare, &archive.manifest).is_err());
    }

    #[test]
    fn archive_publication_is_atomic() {
        let temp = tempfile::tempdir().unwrap();
        let root = host_root(temp.path());
        let path = fixture(temp.path(), |_, _| {});
        let artifact = stage(&root, "boot", &path).unwrap();
        let archive = artifact.archive.as_ref().unwrap();
        let metadata = archive.metadata(&root);
        assert_eq!(archive.sha256, loader::hash(&path).unwrap());
        assert_ne!(artifact.sha256, archive.sha256);
        assert!(!metadata.root.exists());
        let html = archive.dir.join("ui/index.html");
        let original = fs::read(&html).unwrap();
        fs::write(&html, b"corrupt staged UI").unwrap();
        assert!(loader::package(&root, &artifact, &archive.manifest).is_err());
        assert!(
            !metadata.root.exists(),
            "failed publication exposed a digest directory"
        );
        fs::write(html, original).unwrap();
        let published = loader::package(&root, &artifact, &archive.manifest).unwrap();
        assert_eq!(published, metadata.root.join(&metadata.native_path));
        assert_eq!(loader::hash(&published).unwrap(), artifact.sha256);
        verify_directory(&metadata.root, &archive.descriptor, &archive.manifest).unwrap();
        assert!(
            !archive.dir.exists(),
            "publication must move, not partially copy"
        );
        let restarted = stage_published(&root, "restart", &published, &metadata).unwrap();
        assert_eq!(restarted.sha256, artifact.sha256);
        assert_eq!(restarted.archive.as_ref().unwrap().sha256, metadata.digest);
        loader::validate_candidate(&restarted, &archive.manifest).unwrap();
        let duplicate = stage(&root, "boot", &path).unwrap();
        assert_eq!(
            loader::package(&root, &duplicate, &archive.manifest).unwrap(),
            published
        );
        fs::write(metadata.root.join("ui/unlisted.js"), b"extra").unwrap();
        assert!(stage_published(&root, "restart", &published, &metadata).is_err());
        assert!(loader::package(&root, &duplicate, &archive.manifest).is_err());
    }

    #[test]
    fn legacy_registry_and_library_still_load() {
        let profile = tempfile::tempdir().unwrap();
        let store = registry::Store::open(profile.path().to_path_buf()).unwrap();
        let source = store.root.join(format!("legacy.{}", loader::extension()));
        fs::write(&source, b"legacy native fixture").unwrap();
        let artifact = loader::stage(&store.root, "legacy", &source).unwrap();
        let manifest = manifest(false);
        loader::validate_candidate(&artifact, &manifest).unwrap();
        let published = loader::package(&store.root, &artifact, &manifest).unwrap();
        let mut registry = registry::Registry::default();
        registry.plugins.push(registry::RegistryEntry {
            id: manifest.id.clone(),
            manifest,
            source_path: source,
            artifact_path: published,
            sha256: artifact.sha256.clone(),
            package: None,
            desired_enabled: true,
            config_revision: 1,
            config_overrides: ConfigValues::new(),
            quarantine: None,
        });
        let old = serde_json::to_value(&registry).unwrap();
        assert!(
            old["plugins"][0].get("package").is_none(),
            "legacy serialization gained a null field"
        );
        store.write(&registry).unwrap();
        let loaded = store.read().unwrap();
        let entry = &loaded.plugins[0];
        assert!(entry.package.is_none());
        let restarted = loader::stage(&store.root, "restart", &entry.artifact_path).unwrap();
        loader::validate_candidate(&restarted, &entry.manifest).unwrap();
        assert!(restarted.archive.is_none());
        assert_eq!(restarted.sha256, entry.sha256);
    }

    #[cfg(unix)]
    #[test]
    fn archive_rejects_symlinked_profile_and_published_paths() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = host_root(temp.path());
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let path = fixture(temp.path(), |_, _| {});
        symlink(&outside, root.join("staging")).unwrap();
        assert!(stage(&root, "boot", &path).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_file(root.join("staging")).unwrap();
        let artifact = stage(&root, "boot", &path).unwrap();
        let archive = artifact.archive.as_ref().unwrap();
        let metadata = archive.metadata(&root);
        symlink(&outside, root.join("packages")).unwrap();
        assert!(loader::package(&root, &artifact, &archive.manifest).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_file(root.join("packages")).unwrap();
        let published = loader::package(&root, &artifact, &archive.manifest).unwrap();
        let html = metadata.root.join("ui/index.html");
        fs::remove_file(&html).unwrap();
        fs::write(outside.join("index.html"), b"outside").unwrap();
        symlink(outside.join("index.html"), html).unwrap();
        assert!(stage_published(&root, "restart", &published, &metadata).is_err());
    }
}
