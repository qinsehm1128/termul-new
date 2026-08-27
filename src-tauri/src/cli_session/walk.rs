use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::types::WALK_LIMIT_PER_AGENT;

#[derive(Debug, Clone)]
pub struct WalkedFile {
    pub path: PathBuf,
    pub modified: SystemTime,
}

pub fn walk_session_files(
    root: &Path,
    extensions: &[&str],
    skip_dir_names: &[&str],
    extra_filter: Option<&dyn Fn(&Path) -> bool>,
    limit: usize,
) -> Vec<WalkedFile> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return out;
    }
    walk_inner(
        root,
        extensions,
        skip_dir_names,
        extra_filter,
        limit.min(WALK_LIMIT_PER_AGENT),
        0,
        &mut out,
    );
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

fn walk_inner(
    dir: &Path,
    extensions: &[&str],
    skip_dir_names: &[&str],
    extra_filter: Option<&dyn Fn(&Path) -> bool>,
    limit: usize,
    depth: usize,
    out: &mut Vec<WalkedFile>,
) {
    if out.len() >= limit || depth > 12 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if skip_dir_names.iter().any(|skip| *skip == name) {
                continue;
            }
            walk_inner(
                &path,
                extensions,
                skip_dir_names,
                extra_filter,
                limit,
                depth + 1,
                out,
            );
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if let Some(filter) = extra_filter {
            if !filter(&path) {
                continue;
            }
        }
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if !extensions
            .iter()
            .any(|wanted| ext.eq_ignore_ascii_case(wanted))
        {
            continue;
        }
        let modified = fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        out.push(WalkedFile { path, modified });
    }
}

pub fn is_cursor_transcript(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "agent-transcripts")
}

pub fn is_opencode_session_json(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".db") {
        return false;
    }
    path.components().any(|component| {
        let value = component.as_os_str();
        value == "session" || value == "sessions"
    }) || name.starts_with("ses_")
}
