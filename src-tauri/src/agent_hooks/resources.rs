//! Read-only adapter resources. Loading never seeds custom files or executes CLIs.
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub const RESERVED_IDS: [&str; 3] = ["claude-code", "pi", "qin-code"];
pub const MAX_RESOURCE_BYTES: usize = 1024 * 1024;

pub trait AdapterResourceReader: Send + Sync {
    fn enumerate(&self) -> Result<Vec<String>, String>;
    fn read(&self, id: &str, path: &str) -> Result<Vec<u8>, String>;
}

#[derive(rust_embed::RustEmbed)]
#[folder = "resources/agent-hook-adapters/"]
struct EmbeddedResources;
#[derive(Default)]
pub struct EmbeddedAdapterReader;
impl AdapterResourceReader for EmbeddedAdapterReader {
    fn enumerate(&self) -> Result<Vec<String>, String> {
        // Reserved ids remain visible even if one resource is accidentally absent.
        Ok(RESERVED_IDS.iter().map(|id| id.to_string()).collect())
    }
    fn read(&self, id: &str, path: &str) -> Result<Vec<u8>, String> {
        validate_id(id)?;
        validate_relative(path)?;
        EmbeddedResources::get(&format!("{id}/{path}"))
            .map(|file| file.data.into_owned())
            .ok_or_else(|| "missing adapter resource".into())
    }
}

#[derive(Clone, Debug)]
pub struct FilesystemAdapterReader {
    pub root: PathBuf,
}
impl FilesystemAdapterReader {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    /// Tauri maps the directory to resource_dir/agent-hook-adapters on every platform.
    pub fn from_resource_dir(root: impl AsRef<Path>) -> Self {
        Self::new(root.as_ref().join("agent-hook-adapters"))
    }
}
impl AdapterResourceReader for FilesystemAdapterReader {
    fn enumerate(&self) -> Result<Vec<String>, String> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        reject_symlink_components(&self.root)?;
        let mut ids = BTreeSet::new();
        for entry in fs::read_dir(&self.root).map_err(|_| "cannot enumerate adapter root")? {
            let entry = entry.map_err(|_| "cannot enumerate adapter directory")?;
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| "non UTF-8 adapter id")?;
            // Return symlink and malformed directories too: loader reports per-id diagnostics.
            if entry
                .file_type()
                .map_err(|_| "cannot inspect adapter directory")?
                .is_dir()
                || entry
                    .file_type()
                    .map_err(|_| "cannot inspect adapter directory")?
                    .is_symlink()
            {
                ids.insert(id);
            }
        }
        Ok(ids.into_iter().collect())
    }
    fn read(&self, id: &str, path: &str) -> Result<Vec<u8>, String> {
        validate_id(id)?;
        validate_relative(path)?;
        let file = open_no_follow(&self.root.join(id).join(path))?;
        if !file
            .metadata()
            .map_err(|_| "cannot inspect resource")?
            .is_file()
        {
            return Err("adapter resource must be a regular file".into());
        }
        let mut bytes = Vec::new();
        file.take((MAX_RESOURCE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read adapter resource")?;
        if bytes.len() > MAX_RESOURCE_BYTES {
            return Err("adapter resource exceeds limit".into());
        }
        Ok(bytes)
    }
}

/// Server has no runtime dependency on Tauri. Desktop explicitly supplies its resource directory.
static BUILTIN_RESOURCE_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
pub fn set_builtin_resource_dir(resource_dir: PathBuf) -> Result<(), String> {
    BUILTIN_RESOURCE_DIR
        .set(resource_dir)
        .map_err(|_| "builtin resource directory already initialized".into())
}
pub fn builtin_reader() -> Box<dyn AdapterResourceReader> {
    match BUILTIN_RESOURCE_DIR.get() {
        Some(root) => desktop_builtin_reader(root),
        None => Box::new(EmbeddedAdapterReader),
    }
}
pub fn desktop_builtin_reader(resource_dir: impl AsRef<Path>) -> Box<dyn AdapterResourceReader> {
    Box::new(FilesystemAdapterReader::from_resource_dir(resource_dir))
}

pub fn validate_id(id: &str) -> Result<(), String> {
    if id.len() > 64
        || id.is_empty()
        || !id.as_bytes()[0].is_ascii_lowercase()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("invalid adapter identifier".into());
    }
    Ok(())
}
pub fn validate_relative(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains(['\\', ':', '\0'])
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("unsafe relative adapter path".into());
    }
    Ok(())
}
pub fn reject_symlink_components(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err("unsafe path".into());
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("symlink adapter path rejected".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("cannot inspect adapter path".into()),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> Result<fs::File, String> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    // Walk from a held directory handle, never follow links, including the root's ancestors.
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| "cannot resolve adapter root")?
            .join(path)
    };
    let mut dir = fs::File::open("/").map_err(|_| "cannot open filesystem root")?;
    let parts: Vec<_> = absolute
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(p) => Some(Ok(p)),
            std::path::Component::RootDir => None,
            _ => Some(Err("unsafe adapter path")),
        })
        .collect::<Result<_, _>>()?;
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(part.as_bytes()).map_err(|_| "unsafe adapter path")?;
        let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
        if index + 1 != parts.len() {
            flags |= libc::O_DIRECTORY;
        }
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err("cannot open adapter resource without following links".into());
        }
        dir = unsafe { fs::File::from_raw_fd(fd) };
    }
    Ok(dir)
}
#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> Result<fs::File, String> {
    reject_symlink_components(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        return fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x00200000)
            .open(path)
            .map_err(|_| "cannot open adapter resource".into());
    }
    #[cfg(not(windows))]
    fs::File::open(path).map_err(|_| "cannot open adapter resource".into())
}
