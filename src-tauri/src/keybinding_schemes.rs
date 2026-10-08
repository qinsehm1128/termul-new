//! User keybinding scheme files: `~/<workspace dir>/keybindings/*.json`.
//!
//! The renderer owns parsing and validation; this side only lists and reads
//! the files so a broken one can be reported by name instead of failing the
//! whole load.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::commands::IpcResult;

/// Larger files are not keybinding schemes; skip them rather than read them.
const MAX_SCHEME_BYTES: u64 = 256 * 1024;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchemeFile {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemeDirListing {
    dir: String,
    files: Vec<SchemeFile>,
}

fn schemes_dir() -> Option<PathBuf> {
    se_mcp_bridge::settings::home_dir().map(|home| {
        home.join(crate::brand::canonical().workspace_dir)
            .join("keybindings")
    })
}

fn read_scheme_file(path: &Path) -> SchemeFile {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let read = fs::metadata(path).and_then(|meta| {
        if meta.len() > MAX_SCHEME_BYTES {
            Err(std::io::Error::other("file is too large"))
        } else {
            fs::read_to_string(path)
        }
    });
    match read {
        Ok(content) => SchemeFile {
            name,
            content: Some(content),
            error: None,
        },
        Err(error) => SchemeFile {
            name,
            content: None,
            error: Some(error.to_string()),
        },
    }
}

/// The `*.json` files directly in `dir`, sorted by name. A missing directory
/// is an empty listing.
fn list_scheme_files(dir: &Path) -> std::io::Result<Vec<SchemeFile>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        })
        .collect();
    paths.sort();
    Ok(paths.iter().map(|path| read_scheme_file(path)).collect())
}

/// List user scheme files. `ensure_dir` creates the directory first, for the
/// "open folder" action.
#[tauri::command]
pub async fn keybinding_schemes_load(ensure_dir: bool) -> IpcResult<SchemeDirListing> {
    let Some(dir) = schemes_dir() else {
        return IpcResult::error("No home directory", "NO_HOME_DIR");
    };
    let result = tokio::task::spawn_blocking(move || {
        if ensure_dir {
            fs::create_dir_all(&dir)?;
        }
        list_scheme_files(&dir).map(|files| SchemeDirListing {
            dir: dir.to_string_lossy().into_owned(),
            files,
        })
    })
    .await;
    match result {
        Ok(Ok(listing)) => IpcResult::success(listing),
        Ok(Err(error)) => IpcResult::error(error.to_string(), "KEYBINDING_SCHEMES_READ_FAILED"),
        Err(_) => IpcResult::error("scheme load task failed", "KEYBINDING_SCHEMES_READ_FAILED"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_only_json_files_sorted_and_reports_unreadable_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("b.json"), "{\"bindings\":{}}").unwrap();
        fs::write(dir.path().join("a.JSON"), "{}").unwrap();
        fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        fs::create_dir(dir.path().join("nested.json")).unwrap();
        fs::write(
            dir.path().join("huge.json"),
            vec![b' '; (MAX_SCHEME_BYTES + 1) as usize],
        )
        .unwrap();

        let files = list_scheme_files(dir.path()).expect("listing");
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, ["a.JSON", "b.json", "huge.json"]);
        assert_eq!(files[1].content.as_deref(), Some("{\"bindings\":{}}"));
        assert!(files[2].content.is_none());
        assert!(files[2].error.is_some());
    }

    #[test]
    fn a_missing_directory_is_an_empty_listing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let files = list_scheme_files(&dir.path().join("absent")).expect("listing");
        assert!(files.is_empty());
    }
}
