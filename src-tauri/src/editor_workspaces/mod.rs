//! Discover recent VS Code-family and Zed workspaces on the host.

mod code_workspace;
mod discover;
mod normalize;
mod vscode;
mod zed;

pub use code_workspace::parse_code_workspace_file;
pub use discover::{discover_editor_workspaces, EditorWorkspaceList};
pub use normalize::{decode_location, EditorWorkspaceKind};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorWorkspaceCandidate {
    pub id: String,
    pub editor: EditorWorkspaceKind,
    pub name: String,
    pub path: String,
    pub source: EditorWorkspaceSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditorWorkspaceSource {
    Recent,
    WorkspaceFile,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_file(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }

    fn file_uri(path: &std::path::Path) -> String {
        url::Url::from_file_path(path)
            .expect("absolute fixture path")
            .to_string()
    }

    #[test]
    fn decodes_file_uri_and_tilde() {
        let folder = TempDir::new().unwrap();
        let decoded = decode_location(&file_uri(folder.path())).expect("file uri");
        assert_eq!(decoded, folder.path());
        let home = super::normalize::user_home().expect("home");
        assert_eq!(decode_location("~/proj").unwrap(), home.join("proj"));
    }

    #[test]
    fn parse_code_workspace_resolves_relative_folders() {
        let root = TempDir::new().unwrap();
        let alpha = root.path().join("alpha");
        let beta = root.path().join("beta");
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        let workspace = root.path().join("app.code-workspace");
        write_file(
            &workspace,
            r#"{
              "folders": [
                { "path": "alpha", "name": "Alpha App" },
                { "path": "./beta" },
                { "path": "missing" }
              ]
            }"#,
        );

        let list = parse_code_workspace_file(&workspace).expect("parse workspace");
        assert!(parse_code_workspace_file(&alpha)
            .unwrap_err()
            .contains(".code-workspace"));
        let paths: Vec<_> = list.candidates.iter().map(|c| c.path.clone()).collect();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|path| path.ends_with("alpha")));
        assert!(paths.iter().any(|path| path.ends_with("beta")));
        assert!(list
            .candidates
            .iter()
            .all(|c| c.source == EditorWorkspaceSource::WorkspaceFile));
        assert_eq!(
            list.candidates
                .iter()
                .find(|c| c.path.ends_with("alpha"))
                .map(|c| c.name.as_str()),
            Some("Alpha App")
        );
    }

    #[test]
    fn vscode_storage_collects_folders_and_workspace_files() {
        let home = TempDir::new().unwrap();
        let project = home.path().join("proj");
        let nested = home.path().join("multi").join("one");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&nested).unwrap();
        let workspace = home.path().join("multi.code-workspace");
        write_file(&workspace, r#"{"folders":[{"path":"multi/one"}]}"#);
        let support = home.path().join("Library/Application Support");
        write_file(
            &support.join("Code").join("storage.json"),
            &format!(
                r#"{{
                  "backupWorkspaces": {{
                    "folders": [{{ "folderUri": "{}" }}],
                    "workspaces": [{{ "configURIPath": "{}" }}]
                  }}
                }}"#,
                file_uri(&project),
                file_uri(&workspace)
            ),
        );

        let list = vscode::discover_from_roots(
            &[("Code".into(), EditorWorkspaceKind::Vscode)],
            &[support],
        );
        let parsed = parse_code_workspace_file(&workspace).expect("workspace fixture");
        assert!(
            parsed.candidates.iter().any(|c| c.name == "one"),
            "workspace file should resolve multi/one"
        );
        let names: Vec<_> = list.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"proj"), "discovered={names:?}");
        assert!(names.contains(&"one"), "discovered={names:?}");
    }

    #[test]
    fn vscode_storage_dedupes_normalized_paths() {
        let home = TempDir::new().unwrap();
        let project = home.path().join("same");
        fs::create_dir_all(&project).unwrap();
        let support = home.path().join("app-support");
        write_file(
            &support.join("Cursor").join("storage.json"),
            &format!(
                r#"{{
                  "backupWorkspaces": {{
                    "folders": [
                      {{ "folderUri": "{}" }},
                      {{ "folderUri": "{}" }}
                    ]
                  }}
                }}"#,
                file_uri(&project),
                file_uri(&project)
            ),
        );

        let list = vscode::discover_from_roots(
            &[("Cursor".into(), EditorWorkspaceKind::Cursor)],
            &[support],
        );
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].editor, EditorWorkspaceKind::Cursor);
    }

    #[test]
    fn zed_paths_split_on_newlines() {
        let home = TempDir::new().unwrap();
        let a = home.path().join("zed-a");
        let b = home.path().join("zed-b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let db_path = home.path().join("db.sqlite");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE workspaces (workspace_id INTEGER PRIMARY KEY, paths TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workspaces (paths) VALUES (?1)",
            [format!("{}\n{}", a.display(), b.display())],
        )
        .unwrap();

        let list = zed::discover_from_db(&db_path);
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|c| c.editor == EditorWorkspaceKind::Zed));
        assert!(list
            .iter()
            .all(|c| c.source == EditorWorkspaceSource::Recent));
    }

    #[test]
    fn skips_missing_directories() {
        let missing = std::env::temp_dir().join("termul-missing-editor-workspace-dir");
        let _ = fs::remove_dir_all(&missing);
        let list = discover::collect_folder(
            EditorWorkspaceKind::Vscode,
            EditorWorkspaceSource::Recent,
            &missing,
            None,
        );
        assert!(list.is_empty());
    }
}
