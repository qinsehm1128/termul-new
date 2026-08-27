use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::normalize::EditorWorkspaceKind;
use super::{discover, EditorWorkspaceCandidate, EditorWorkspaceSource};

pub fn discover() -> Vec<EditorWorkspaceCandidate> {
    let mut collected = Vec::new();
    for db in zed_db_paths() {
        collected.extend(discover_from_db(&db));
    }
    collected
}

pub fn discover_from_db(db_path: &Path) -> Vec<EditorWorkspaceCandidate> {
    let Ok(conn) = open_readonly(db_path) else {
        return Vec::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT paths FROM workspaces WHERE paths IS NOT NULL AND length(paths) > 0")
    else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |row| row.get::<_, String>(0));
    let Ok(rows) = rows else {
        return Vec::new();
    };
    let mut collected = Vec::new();
    for row in rows.flatten() {
        for raw in row.split('\n') {
            if let Some(path) = super::normalize::decode_location(raw) {
                collected.extend(discover::collect_folder(
                    EditorWorkspaceKind::Zed,
                    EditorWorkspaceSource::Recent,
                    &path,
                    None,
                ));
            }
        }
    }
    collected
}

fn open_readonly(path: &Path) -> rusqlite::Result<Connection> {
    let uri = format!("file:{}?mode=ro", path.to_string_lossy().replace('\\', "/"));
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
}

fn zed_db_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = super::normalize::user_home() {
        paths.push(
            home.join("Library")
                .join("Application Support")
                .join("Zed")
                .join("db")
                .join("0-stable")
                .join("db.sqlite"),
        );
        paths.push(
            home.join(".local")
                .join("share")
                .join("zed")
                .join("db")
                .join("0-stable")
                .join("db.sqlite"),
        );
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            paths.push(
                PathBuf::from(xdg)
                    .join("zed")
                    .join("db")
                    .join("0-stable")
                    .join("db.sqlite"),
            );
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        paths.push(
            PathBuf::from(appdata)
                .join("Zed")
                .join("db")
                .join("0-stable")
                .join("db.sqlite"),
        );
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local)
                .join("Zed")
                .join("db")
                .join("0-stable")
                .join("db.sqlite"),
        );
    }
    paths
}
