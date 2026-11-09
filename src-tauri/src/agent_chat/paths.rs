//! Host-only transcript resolution. A client supplies terminal identity, never a file path.
use crate::{
    agent_hooks::{AgentHookEvent, AgentKind},
    cli_session::paths,
};
use std::{
    collections::VecDeque,
    fs::File,
    io,
    path::{Path, PathBuf},
};

fn qin_home() -> Option<PathBuf> {
    std::env::var_os("QIN_CODE_HOME")
        .map(PathBuf::from)
        .or_else(|| paths::user_home().map(|p| p.join(".qin-code")))
}

pub fn resolve(event: &AgentHookEvent) -> Option<PathBuf> {
    resolve_with(
        event,
        paths::claude_projects_dir(),
        paths::pi_sessions_dir(),
        qin_home(),
    )
}

fn resolve_with(
    event: &AgentHookEvent,
    claude: Option<PathBuf>,
    pi: Option<PathBuf>,
    qin: Option<PathBuf>,
) -> Option<PathBuf> {
    // Hook normalization already selects transcript_path, then native session_file.
    if let Some(path) = &event.transcript_path {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            if event.agent == AgentKind::QinCode
                && path.extension().is_some_and(|ext| ext == "json")
            {
                return Some(path.with_extension("journal.jsonl"));
            }
            return Some(path);
        }
    }
    let id = &event.session_id;
    if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
        return None;
    }
    match event.agent {
        AgentKind::ClaudeCode => find(claude?, |path| {
            path.file_name().and_then(|s| s.to_str()) == Some(&format!("{id}.jsonl"))
        }),
        AgentKind::Pi => find(pi?, |path| {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            crate::memory_index::adapters::pi::is_transcript_name(name)
                && (name == format!("{id}.jsonl") || name.ends_with(&format!("_{id}.jsonl")))
        }),
        AgentKind::QinCode => {
            let root = qin?.join("sessions").canonicalize().ok()?;
            let flat = root.join(format!("{id}.journal.jsonl"));
            if flat.is_file() {
                return contained_file(&root, &flat).ok();
            }
            let direct = root.join(id).join("journal.jsonl");
            if direct.is_file() {
                return contained_file(&root, &direct).ok();
            }
            let candidate = find(root.clone(), |path| {
                path.file_name().is_some_and(|s| s == "journal.jsonl")
                    && path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|s| s.to_str())
                        == Some(id.as_str())
            })?;
            contained_file(&root, &candidate).ok()
        }
    }
}

fn contained_file(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let path = path.canonicalize()?;
    if !path.starts_with(root) || !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Qin journal is outside the approved sessions store",
        ));
    }
    Ok(path)
}

/// Verify the actual file handle before reading a Qin fallback journal.
///
/// Call immediately after `File::open(path)` and before reading from the file.
/// Resolving a path is not authorization for a later open: a journal or one of
/// its parent directories may have been replaced by a symlink in between.
/// Explicit absolute hook paths retain their separate trusted-path contract.
/// Unsupported handle-path platforms fail closed for Qin fallback reads.
pub fn verify_opened(event: &AgentHookEvent, path: &Path, file: &File) -> io::Result<()> {
    verify_opened_with(event, path, file, qin_home())
}

fn verify_opened_with(
    event: &AgentHookEvent,
    path: &Path,
    file: &File,
    qin: Option<PathBuf>,
) -> io::Result<()> {
    if event.agent != AgentKind::QinCode
        || event
            .transcript_path
            .as_deref()
            .is_some_and(|p| Path::new(p).is_absolute())
    {
        return Ok(());
    }
    let root = qin
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Qin home is unavailable"))?
        .join("sessions")
        .canonicalize()?;
    let candidate = contained_file(&root, path)?;
    let opened = opened_path(file)?;
    // The kernel already resolved the handle path. Canonicalizing it again
    // could follow a newly swapped symlink instead of checking the open object.
    if !opened.starts_with(&root) || opened != candidate || !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Opened Qin journal does not match the approved sessions store",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let expected = std::fs::metadata(&candidate)?;
        let actual = file.metadata()?;
        if expected.dev() != actual.dev() || expected.ino() != actual.ino() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Opened Qin journal changed during validation",
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn opened_path(file: &File) -> io::Result<PathBuf> {
    use std::{ffi::CStr, os::fd::AsRawFd, os::unix::ffi::OsStrExt};
    let mut buffer = [0 as libc::c_char; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes a NUL-terminated path to a PATH_MAX-sized buffer;
    // the borrowed File keeps the descriptor alive for the duration of the call.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful F_GETPATH guarantees a terminating NUL in the buffer.
    let bytes = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_bytes();
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(target_os = "linux")]
fn opened_path(file: &File) -> io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(windows)]
fn opened_path(file: &File) -> io::Result<PathBuf> {
    use std::{os::windows::ffi::OsStringExt, os::windows::io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let handle = file.as_raw_handle();
    // SAFETY: querying the required length does not dereference the null buffer.
    let size = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
    if size == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0u16; size as usize + 1];
    // SAFETY: buffer owns enough initialized u16s and the File keeps handle live.
    let written =
        unsafe { GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0) };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    if written as usize >= buffer.len() {
        return Err(io::Error::other(
            "Opened Qin journal path changed during validation",
        ));
    }
    Ok(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..written as usize],
    )))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn opened_path(_: &File) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Opened Qin journal path validation is unavailable on this platform",
    ))
}

fn find(root: PathBuf, matches: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut queue = VecDeque::from([(root, 0u8)]);
    let mut visited = 0;
    let mut found = None;
    while let Some((dir, depth)) = queue.pop_front() {
        if depth > 8 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > 50_000 {
                return None;
            } // ambiguity/bound exceeded, never guess
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                queue.push_back((path, depth + 1));
            } else if kind.is_file() && matches(&path) {
                if found.is_some() {
                    return None;
                }
                found = Some(path);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_hooks::HookKind;
    fn event(agent: AgentKind, id: &str) -> AgentHookEvent {
        AgentHookEvent {
            sequence: 1,
            terminal_id: "fixture".into(),
            project_id: "fixture".into(),
            agent,
            session_id: id.into(),
            transcript_path: None,
            kind: HookKind::Idle,
            at: 1,
            tool_name: None,
            ok: None,
        }
    }
    fn file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    #[test]
    fn hook_path_wins_and_qin_snapshot_maps_to_native_journal() {
        let root = tempfile::tempdir().unwrap();
        let mut e = event(AgentKind::Pi, "id");
        let path = root.path().join("explicit.jsonl");
        e.transcript_path = Some(path.to_string_lossy().into());
        assert_eq!(resolve_with(&e, None, None, None), Some(path));
        e.agent = AgentKind::QinCode;
        e.transcript_path = Some(root.path().join("session.json").to_string_lossy().into());
        assert_eq!(
            resolve_with(&e, None, None, None),
            Some(root.path().join("session.journal.jsonl"))
        );
    }
    #[test]
    fn resolves_native_claude_and_pi_ids_in_isolated_stores() {
        let root = tempfile::tempdir().unwrap();
        let c = root.path().join("claude");
        let p = root.path().join("pi");
        let claude = c.join("-fixture-project").join("session-id.jsonl");
        file(&claude);
        assert_eq!(
            resolve_with(
                &event(AgentKind::ClaudeCode, "session-id"),
                Some(c),
                None,
                None
            ),
            Some(claude)
        );
        let id = "b4371092-001c-44ba-a423-3d8e7b1d3bc0";
        let pi = p
            .join("--fixture-project--")
            .join(format!("2026-11-09T10-00-00-000Z_{id}.jsonl"));
        file(&pi);
        assert_eq!(
            resolve_with(&event(AgentKind::Pi, id), None, Some(p), None),
            Some(pi)
        );
    }
    #[test]
    fn resolves_native_flat_and_nested_qin_journals() {
        let root = tempfile::tempdir().unwrap();
        let flat = root.path().join("sessions/id.journal.jsonl");
        file(&flat);
        assert_eq!(
            resolve_with(
                &event(AgentKind::QinCode, "id"),
                None,
                None,
                Some(root.path().into())
            ),
            Some(flat.canonicalize().unwrap())
        );
        let nested = root.path().join("sessions/other/journal.jsonl");
        file(&nested);
        assert_eq!(
            resolve_with(
                &event(AgentKind::QinCode, "other"),
                None,
                None,
                Some(root.path().into())
            ),
            Some(nested.canonicalize().unwrap())
        );
    }
    #[test]
    #[cfg(unix)]
    fn qin_fallback_rejects_symlink_journal_escape() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let qin = root.path().join("qin");
        let outside = root.path().join("outside.jsonl");
        file(&outside);
        std::fs::create_dir_all(qin.join("sessions")).unwrap();
        symlink(&outside, qin.join("sessions/id.journal.jsonl")).unwrap();
        assert!(resolve_with(
            &event(AgentKind::QinCode, "id"),
            None,
            None,
            Some(qin.clone())
        )
        .is_none());
        std::fs::remove_file(qin.join("sessions/id.journal.jsonl")).unwrap();
        std::fs::create_dir_all(qin.join("sessions/id")).unwrap();
        symlink(&outside, qin.join("sessions/id/journal.jsonl")).unwrap();
        assert!(resolve_with(
            &event(AgentKind::QinCode, "id"),
            None,
            None,
            Some(qin.clone())
        )
        .is_none());
        std::fs::rename(qin.join("sessions/id"), qin.join("sessions/project")).unwrap();
        std::fs::create_dir_all(qin.join("sessions/project/id")).unwrap();
        symlink(&outside, qin.join("sessions/project/id/journal.jsonl")).unwrap();
        assert!(resolve_with(&event(AgentKind::QinCode, "id"), None, None, Some(qin)).is_none());
    }

    #[test]
    #[cfg(unix)]
    fn qin_fallback_rejects_symlink_directory_escape() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let qin = root.path().join("qin");
        // A shared string prefix is not containment.
        let outside = qin.join("sessions-outside");
        file(&outside.join("journal.jsonl"));
        std::fs::create_dir_all(qin.join("sessions")).unwrap();
        symlink(&outside, qin.join("sessions/id")).unwrap();
        assert!(resolve_with(
            &event(AgentKind::QinCode, "id"),
            None,
            None,
            Some(qin.clone())
        )
        .is_none());
        std::fs::remove_file(qin.join("sessions/id")).unwrap();
        file(&outside.join("id/journal.jsonl"));
        symlink(&outside, qin.join("sessions/project")).unwrap();
        assert!(resolve_with(&event(AgentKind::QinCode, "id"), None, None, Some(qin)).is_none());
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn qin_opened_file_verification_accepts_canonical_root_and_internal_symlinks() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real-sessions");
        let qin = root.path().join("qin");
        file(&real.join("actual.jsonl"));
        std::fs::create_dir_all(&qin).unwrap();
        symlink(&real, qin.join("sessions")).unwrap();
        symlink("actual.jsonl", real.join("id.journal.jsonl")).unwrap();
        let e = event(AgentKind::QinCode, "id");
        let path = resolve_with(&e, None, None, Some(qin.clone())).unwrap();
        assert_eq!(path, real.join("actual.jsonl").canonicalize().unwrap());
        let opened = File::open(&path).unwrap();
        verify_opened_with(&e, &path, &opened, Some(qin)).unwrap();
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn qin_opened_file_verification_rejects_symlink_swap_and_restored_path() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let qin = root.path().join("qin");
        let outside = root.path().join("outside.jsonl");
        let journal = qin.join("sessions/id.journal.jsonl");
        file(&outside);
        file(&journal);
        let e = event(AgentKind::QinCode, "id");
        let path = resolve_with(&e, None, None, Some(qin.clone())).unwrap();
        std::fs::remove_file(&journal).unwrap();
        symlink(&outside, &journal).unwrap();
        let opened = File::open(&path).unwrap();
        assert!(verify_opened_with(&e, &path, &opened, Some(qin.clone())).is_err());
        // Rechecking only the candidate would now pass, but the opened handle
        // still refers to the outside file and must be rejected.
        std::fs::remove_file(&journal).unwrap();
        file(&journal);
        assert!(verify_opened_with(&e, &path, &opened, Some(qin)).is_err());
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn qin_opened_file_verification_rejects_directory_swap() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let qin = root.path().join("qin");
        let outside = root.path().join("outside");
        file(&outside.join("journal.jsonl"));
        file(&qin.join("sessions/id/journal.jsonl"));
        let e = event(AgentKind::QinCode, "id");
        let path = resolve_with(&e, None, None, Some(qin.clone())).unwrap();
        std::fs::rename(qin.join("sessions/id"), qin.join("sessions/original")).unwrap();
        symlink(&outside, qin.join("sessions/id")).unwrap();
        let opened = File::open(&path).unwrap();
        assert!(verify_opened_with(&e, &path, &opened, Some(qin.clone())).is_err());
        std::fs::remove_file(qin.join("sessions/id")).unwrap();
        std::fs::rename(qin.join("sessions/original"), qin.join("sessions/id")).unwrap();
        assert!(verify_opened_with(&e, &path, &opened, Some(qin)).is_err());
    }

    #[test]
    fn opened_verification_preserves_explicit_hook_path_contract() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("explicit.jsonl");
        file(&path);
        let opened = File::open(&path).unwrap();
        let mut e = event(AgentKind::QinCode, "id");
        e.transcript_path = Some(path.to_string_lossy().into());
        verify_opened_with(&e, &path, &opened, None).unwrap();
        e.agent = AgentKind::Pi;
        e.transcript_path = None;
        verify_opened_with(&e, &path, &opened, None).unwrap();
    }

    #[test]
    fn ambiguous_ids_and_path_traversal_do_not_guess() {
        let root = tempfile::tempdir().unwrap();
        file(&root.path().join("a/id.jsonl"));
        file(&root.path().join("b/id.jsonl"));
        assert!(resolve_with(
            &event(AgentKind::ClaudeCode, "id"),
            Some(root.path().into()),
            None,
            None
        )
        .is_none());
        assert!(resolve_with(
            &event(AgentKind::ClaudeCode, "../id"),
            Some(root.path().into()),
            None,
            None
        )
        .is_none());
    }
}
