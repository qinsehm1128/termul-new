//! Safe, append-only updates to a project's host-backed `.gitignore`.
//!
//! This service deliberately does not require `.git` to exist and never runs git
//! or commits. It only targets `<cwd>/.gitignore`, preserving existing bytes and
//! appending one ordered block when that exact block is not already at the end.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

const MAX_INCOMING_BYTES: usize = 1024 * 1024;
const MAX_EXISTING_BYTES: u64 = 8 * 1024 * 1024;
#[cfg(any(not(unix), test))]
const GITIGNORE_NAME: &str = ".gitignore";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendOutcome {
    pub appended: bool,
}

/// Append `content` to `<cwd>/.gitignore` without requiring a git repository.
///
/// The existing file is never replaced or truncated before all validation and
/// reads complete. Symlinks, reparse points, hard links and non-regular files
/// are rejected using the opened handle. An OS lock serializes cooperating
/// desktop/server callers across the complete read/check/append operation.
/// This is not a sandbox against other processes that already have permission
/// to rename this directory or add inode aliases during the write. Such hostile
/// concurrent filesystem changes require isolation outside this local API.
pub fn append_ignore_rules(cwd: &Path, content: &str) -> Result<AppendOutcome, String> {
    let incoming = normalize_incoming(content)?;
    if incoming.is_empty() {
        return Ok(AppendOutcome { appended: false });
    }

    let cwd = cwd
        .canonicalize()
        .map_err(|e| format!("failed to resolve project directory: {e}"))?;
    let cwd_meta =
        fs::metadata(&cwd).map_err(|e| format!("failed to inspect project directory: {e}"))?;
    if !cwd_meta.is_dir() {
        return Err("project path is not a directory".to_string());
    }
    let directory = open_project_directory(&cwd)?;
    let mut file = open_target(&directory, &cwd)?;
    fs2::FileExt::lock_exclusive(&file).map_err(|e| format!("failed to lock .gitignore: {e}"))?;
    let metadata = validate_handle(&file)?;
    if metadata.len() > MAX_EXISTING_BYTES {
        return Err(".gitignore is too large".to_string());
    }
    let mut existing = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_EXISTING_BYTES + 1)
        .read_to_end(&mut existing)
        .map_err(|e| format!("failed to read .gitignore: {e}"))?;
    if existing.len() as u64 > MAX_EXISTING_BYTES {
        return Err(".gitignore is too large".to_string());
    }

    let last_newline = existing.iter().rposition(|byte| *byte == b'\n');
    let eol = if last_newline.is_some_and(|index| index > 0 && existing[index - 1] == b'\r') {
        b"\r\n".as_slice()
    } else {
        b"\n".as_slice()
    };
    let block = convert_line_endings(&incoming, eol);
    let suffix = if existing.is_empty() || existing.ends_with(b"\n") {
        block.clone()
    } else {
        // Complete a trailing CR with LF instead of producing CRCRLF.
        let mut boundary = if existing.ends_with(b"\r") {
            b"\n".to_vec()
        } else {
            eol.to_vec()
        };
        boundary.extend_from_slice(&block);
        boundary
    };

    let block_start = existing.len().checked_sub(block.len());
    if existing.ends_with(&block)
        && block_start.is_some_and(|start| start == 0 || existing[start - 1] == b'\n')
    {
        return Ok(AppendOutcome { appended: false });
    }

    // Recheck aliases immediately before mutation, not just at path inspection.
    validate_handle(&file)?;
    file.write_all(&suffix)
        .map_err(|e| format!("failed to append .gitignore: {e}"))?;
    file.flush()
        .map_err(|e| format!("failed to flush .gitignore: {e}"))?;

    Ok(AppendOutcome { appended: true })
}

fn normalize_incoming(content: &str) -> Result<Vec<u8>, String> {
    if content.as_bytes().len() > MAX_INCOMING_BYTES {
        return Err("ignore rules are too large".to_string());
    }
    if content.as_bytes().contains(&0) {
        return Err("ignore rules contain NUL".to_string());
    }
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut bytes = normalized.into_bytes();
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn convert_line_endings(input: &[u8], eol: &[u8]) -> Vec<u8> {
    if eol == b"\n" {
        return input.to_vec();
    }
    let mut output =
        Vec::with_capacity(input.len() + input.iter().filter(|&&b| b == b'\n').count());
    for &byte in input {
        if byte == b'\n' {
            output.extend_from_slice(eol);
        } else {
            output.push(byte);
        }
    }
    output
}

fn open_project_directory(cwd: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        // Keep the directory open without delete sharing while the child path
        // is resolved. A junction must not redirect the project directory.
        options
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let directory = options
        .open(cwd)
        .map_err(|e| format!("failed to open project directory: {e}"))?;
    let metadata = directory
        .metadata()
        .map_err(|e| format!("failed to inspect project directory: {e}"))?;
    if !metadata.is_dir() {
        return Err("project path is not a directory".to_string());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("project directory was replaced by a reparse point".to_string());
        }
    }
    Ok(directory)
}

#[cfg(unix)]
fn open_child(directory: &File, _cwd: &Path, create: bool) -> io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let flags =
        libc::O_RDWR | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
    let flags = if create {
        flags | libc::O_CREAT | libc::O_EXCL
    } else {
        flags
    };
    // SAFETY: directory owns its descriptor and the child name is a fixed,
    // NUL-terminated single component. Successful openat transfers a new fd.
    let fd = unsafe { libc::openat(directory.as_raw_fd(), c".gitignore".as_ptr(), flags, 0o666) };
    if fd == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(not(unix))]
fn open_child(_directory: &File, cwd: &Path, create: bool) -> io::Result<File> {
    safe_open_options()
        .create_new(create)
        .open(cwd.join(GITIGNORE_NAME))
}

fn open_target(directory: &File, cwd: &Path) -> Result<File, String> {
    // If another caller creates the file between open and create_new, retry its
    // handle, then lock/read it. Never truncate or write before the lock.
    for _ in 0..4 {
        match open_child(directory, cwd, false) {
            Ok(file) => {
                validate_handle(&file)?;
                return Ok(file);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match open_child(directory, cwd, true) {
                    Ok(file) => {
                        validate_handle(&file)?;
                        return Ok(file);
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(format!("failed to create .gitignore: {error}")),
                }
            }
            Err(error) => return Err(format!("failed to open .gitignore: {error}")),
        }
    }
    Err(".gitignore changed repeatedly while opening".to_string())
}

#[cfg(not(unix))]
fn safe_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).append(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    options
}

fn validate_handle(file: &File) -> Result<Metadata, String> {
    let metadata = file
        .metadata()
        .map_err(|e| format!("failed to inspect .gitignore: {e}"))?;
    if !metadata.is_file() {
        return Err(".gitignore is not a regular file".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(".gitignore hard links are not allowed".to_string());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
        };
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(".gitignore reparse points are not allowed".to_string());
        }
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: file owns a valid handle for this call and the OS initializes
        // the entire output structure on success. No data is read on failure.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
        if ok == 0 {
            return Err(format!(
                "failed to inspect .gitignore links: {}",
                io::Error::last_os_error()
            ));
        }
        let info = unsafe { info.assume_init() };
        if info.nNumberOfLinks != 1 {
            return Err(".gitignore hard links are not allowed".to_string());
        }
    }
    #[cfg(not(any(unix, windows)))]
    return Err("safe .gitignore updates are unsupported on this platform".to_string());

    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("se-gitignore-{label}-{stamp}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run(label: &str, existing: Option<&[u8]>, incoming: &str) -> (AppendOutcome, Vec<u8>) {
        let dir = TempDir::new(label);
        let target = dir.path().join(GITIGNORE_NAME);
        if let Some(bytes) = existing {
            fs::write(&target, bytes).unwrap();
        }
        let result = append_ignore_rules(dir.path(), incoming).unwrap();
        let bytes = fs::read(&target).unwrap_or_default();
        (result, bytes)
    }

    #[test]
    fn absent_file_is_created_and_existing_bytes_are_preserved() {
        let (outcome, bytes) = run("absent", None, "node_modules\ndist");
        assert!(outcome.appended);
        assert_eq!(bytes, b"node_modules\ndist\n");
    }

    #[test]
    fn no_eol_gets_one_boundary_newline() {
        let (outcome, bytes) = run("no-eol", Some(b"# existing"), "foo");
        assert!(outcome.appended);
        assert_eq!(bytes, b"# existing\nfoo\n");
    }

    #[test]
    fn crlf_existing_line_ending_is_preserved() {
        let (_, bytes) = run("crlf", Some(b"base\r\n"), "foo\r\nbar");
        assert_eq!(bytes, b"base\r\nfoo\r\nbar\r\n");
    }

    #[test]
    fn blank_input_does_not_create_file() {
        let dir = TempDir::new("blank");
        let outcome = append_ignore_rules(dir.path(), " \r\n\t").unwrap();
        assert!(!outcome.appended);
        assert!(!dir.path().join(GITIGNORE_NAME).exists());
    }

    #[test]
    fn suffix_inside_another_rule_is_not_a_complete_block() {
        let (outcome, bytes) = run("line-boundary", Some(b"other-cache/\n"), "cache/\n");
        assert!(outcome.appended);
        assert_eq!(bytes, b"other-cache/\ncache/\n");
    }

    #[test]
    fn unterminated_crlf_files_and_trailing_cr_get_real_line_boundaries() {
        let (_, bytes) = run("crlf-no-eol", Some(b"one/\r\ntwo/"), "three/");
        assert_eq!(bytes, b"one/\r\ntwo/\r\nthree/\r\n");
        let (_, bytes) = run("trailing-cr", Some(b"one/\r\ntwo/\r"), "three/");
        assert_eq!(bytes, b"one/\r\ntwo/\r\nthree/\r\n");
        let (outcome, bytes) = run("embedded-cr", Some(b"one/\rcache/\n"), "cache/");
        assert!(outcome.appended);
        assert_eq!(bytes, b"one/\rcache/\ncache/\n");
    }

    #[test]
    fn existing_non_utf8_bytes_remain_an_exact_prefix() {
        let (_, bytes) = run("raw-prefix", Some(b"\xfflegacy/\n"), ".qin-code/");
        assert_eq!(bytes, b"\xfflegacy/\n.qin-code/\n");
    }

    #[test]
    fn concurrent_identical_requests_append_exactly_once_for_new_and_existing_files() {
        use std::sync::{Arc, Barrier};
        for existing in [false, true] {
            let dir = TempDir::new("concurrent");
            let target = dir.path().join(GITIGNORE_NAME);
            if existing {
                fs::write(&target, b"# keep\n").unwrap();
            }
            let barrier = Arc::new(Barrier::new(16));
            let threads: Vec<_> = (0..16)
                .map(|_| {
                    let cwd = dir.path().to_path_buf();
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        append_ignore_rules(&cwd, "cache/\n!cache/keep\n").unwrap()
                    })
                })
                .collect();
            let writes = threads
                .into_iter()
                .map(|thread| usize::from(thread.join().unwrap().appended))
                .sum::<usize>();
            assert_eq!(writes, 1);
            let prefix = if existing { "# keep\n" } else { "" };
            assert_eq!(
                fs::read(&target).unwrap(),
                format!("{prefix}cache/\n!cache/keep\n").as_bytes()
            );
        }
    }

    #[test]
    fn hard_link_is_rejected_without_changing_the_external_inode() {
        let dir = TempDir::new("hard-link");
        let outside = TempDir::new("hard-link-outside");
        let outside_file = outside.path().join("keep");
        fs::write(&outside_file, b"untouched\n").unwrap();
        fs::hard_link(&outside_file, dir.path().join(GITIGNORE_NAME)).unwrap();
        assert!(append_ignore_rules(dir.path(), "changed/").is_err());
        assert_eq!(fs::read(&outside_file).unwrap(), b"untouched\n");
    }

    #[cfg(unix)]
    #[test]
    fn swapped_fifo_is_rejected_without_waiting_for_a_writer() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let dir = TempDir::new("fifo");
        let path = dir.path().join(GITIGNORE_NAME);
        let path_c = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: path_c is a valid NUL-terminated path in this test's temp dir.
        assert_eq!(unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) }, 0);
        assert!(append_ignore_rules(dir.path(), "changed/").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn opened_project_directory_anchors_child_lookup_during_path_replacement() {
        let root = TempDir::new("directory-anchor");
        let project = root.path().join("project");
        let moved = root.path().join("moved");
        let external = root.path().join("external");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&external).unwrap();
        fs::write(project.join(GITIGNORE_NAME), b"original\n").unwrap();
        fs::write(external.join(GITIGNORE_NAME), b"external\n").unwrap();
        let directory = open_project_directory(&project).unwrap();
        fs::rename(&project, &moved).unwrap();
        std::os::unix::fs::symlink(&external, &project).unwrap();
        assert!(open_project_directory(&project).is_err());
        let mut file = open_target(&directory, &project).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(
            bytes, b"original\n",
            "openat must not follow the replacement path"
        );
        assert_eq!(
            fs::read(external.join(GITIGNORE_NAME)).unwrap(),
            b"external\n"
        );
    }

    #[test]
    fn repeated_complete_block_is_idempotent() {
        let dir = TempDir::new("repeat");
        let first = append_ignore_rules(dir.path(), "foo\n!foo/bar").unwrap();
        let second = append_ignore_rules(dir.path(), "foo\n!foo/bar").unwrap();
        assert!(first.appended);
        assert!(!second.appended);
        assert_eq!(
            fs::read(dir.path().join(GITIGNORE_NAME)).unwrap(),
            b"foo\n!foo/bar\n"
        );
    }

    #[test]
    fn ordered_negation_block_is_not_globally_deduplicated() {
        let (outcome, bytes) = run("negation", Some(b"foo\n!foo/bar\n"), "foo\n!foo/bar");
        assert!(!outcome.appended);
        assert_eq!(bytes, b"foo\n!foo/bar\n");
        let (_, bytes) = run("negation-order", Some(b"foo\n"), "!foo/bar\nfoo");
        assert_eq!(bytes, b"foo\n!foo/bar\nfoo\n");
    }

    #[test]
    fn nul_and_oversize_fail_without_creating() {
        let dir = TempDir::new("guards");
        assert!(append_ignore_rules(dir.path(), "bad\0rule").is_err());
        assert!(append_ignore_rules(dir.path(), &"x".repeat(MAX_INCOMING_BYTES + 1)).is_err());
        assert!(!dir.path().join(GITIGNORE_NAME).exists());
    }

    #[test]
    fn symlink_is_rejected_without_touching_outside_target() {
        let dir = TempDir::new("symlink");
        let outside = TempDir::new("outside");
        let outside_file = outside.path().join("outside");
        fs::write(&outside_file, b"do not touch\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_file, dir.path().join(GITIGNORE_NAME)).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside_file, dir.path().join(GITIGNORE_NAME)).unwrap();
        assert!(append_ignore_rules(dir.path(), "changed").is_err());
        assert_eq!(fs::read(outside_file).unwrap(), b"do not touch\n");
    }

    #[test]
    fn directory_target_is_rejected() {
        let dir = TempDir::new("directory");
        fs::create_dir(dir.path().join(GITIGNORE_NAME)).unwrap();
        assert!(append_ignore_rules(dir.path(), "x").is_err());
    }

    #[test]
    fn oversized_existing_file_is_rejected_without_truncation() {
        let dir = TempDir::new("existing-large");
        let target = dir.path().join(GITIGNORE_NAME);
        fs::write(&target, vec![b'x'; (MAX_EXISTING_BYTES + 1) as usize]).unwrap();
        assert!(append_ignore_rules(dir.path(), "y").is_err());
        assert_eq!(fs::metadata(target).unwrap().len(), MAX_EXISTING_BYTES + 1);
    }
}
