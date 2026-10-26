//! Host wiring for quick terminals (`se-quick-terminal`).
//!
//! The service lives next to the PTYs: inside Terminal Core when it runs, in
//! the host process otherwise (Windows desktop, standalone `se-server`). Every
//! caller — Tauri commands, web routes, the Terminal Core request loop — goes
//! through [`dispatch`], so the local and IPC paths answer identically and
//! keep the same stable error codes. This module is shared with Terminal Core;
//! the GUI-side entry point is `quick_terminal_commands`.

use std::path::Path;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::handles::TerminalServiceHandle;
use se_pty::PtyManager;

pub use se_quick_terminal::{
    CreateQuickTerminal, QuickTerminalError, QuickTerminalId, QuickTerminalOpened,
    QuickTerminalRecord, QuickTerminalService, QuickTerminalStore,
};

pub const METHOD_LIST: &str = "quickTerminalList";
pub const METHOD_CREATE: &str = "quickTerminalCreate";
pub const METHOD_OPEN: &str = "quickTerminalOpen";
pub const METHOD_RENAME: &str = "quickTerminalRename";
pub const METHOD_DELETE: &str = "quickTerminalDelete";
pub const METHOD_CLOSE: &str = "quickTerminalClose";
pub const METHOD_CREATE_TERMINAL: &str = "quickTerminalCreateTerminal";
pub const METHOD_CLOSE_TERMINAL: &str = "quickTerminalCloseTerminal";
/// Host-internal: adopt a record migrated from a legacy terminal Conversation.
pub const METHOD_IMPORT: &str = "quickTerminalImport";

pub const UNAVAILABLE: &str = "QUICK_TERMINAL_UNAVAILABLE";
pub const INVALID_REQUEST: &str = "QUICK_TERMINAL_INVALID_REQUEST";

pub fn is_quick_terminal_method(method: &str) -> bool {
    matches!(
        method,
        METHOD_LIST
            | METHOD_CREATE
            | METHOD_OPEN
            | METHOD_RENAME
            | METHOD_DELETE
            | METHOD_CLOSE
            | METHOD_CREATE_TERMINAL
            | METHOD_CLOSE_TERMINAL
            | METHOD_IMPORT
    )
}

/// Open the store under `<profile_root>/quick-terminals`; new private folders
/// go under `<workspace_base>/terminals`.
pub fn open_service(
    profile_root: &Path,
    workspace_base: &Path,
    pty: Arc<PtyManager>,
) -> std::io::Result<Arc<QuickTerminalService>> {
    let store = QuickTerminalStore::open(profile_root.join("quick-terminals"))?;
    Ok(Arc::new(QuickTerminalService::new(
        store,
        workspace_base.to_path_buf(),
        pty,
    )))
}

/// Give an in-process terminal handle (no Terminal Core) its own quick
/// terminal service. A store that cannot be opened is logged and leaves the
/// handle without quick terminals rather than failing startup.
pub fn with_local_service(
    handle: TerminalServiceHandle,
    profile_root: &Path,
    workspace_base: &Path,
) -> TerminalServiceHandle {
    let Some(pty) = handle.in_process_pty() else {
        return handle;
    };
    match open_service(profile_root, workspace_base, pty) {
        Ok(service) => handle.with_quick_terminals(service),
        Err(error) => {
            log::error!(
                target: "se_manager::quick_terminal",
                "operation=store_open stable_code=QUICK_TERMINAL_UNAVAILABLE error={error}"
            );
            handle
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuickTerminalIdParams {
    pub id: QuickTerminalId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenQuickTerminal {
    pub id: QuickTerminalId,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub terminal_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateQuickTerminalTerminal {
    pub id: QuickTerminalId,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloseQuickTerminalTerminal {
    pub id: QuickTerminalId,
    pub terminal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameQuickTerminal {
    pub id: QuickTerminalId,
    #[serde(default)]
    pub title: Option<String>,
}

/// Wire reply between Terminal Core and the GUI. Carries the stable code the
/// generic Core error path would flatten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum QuickTerminalReply {
    Ok { value: Value },
    Err { code: String, message: String },
}

impl QuickTerminalReply {
    pub fn err(code: &str, message: impl Into<String>) -> Self {
        Self::Err {
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn from_result<T: Serialize>(result: Result<T, QuickTerminalError>) -> Self {
        match result {
            Ok(value) => match serde_json::to_value(value) {
                Ok(value) => Self::Ok { value },
                Err(error) => Self::err(INVALID_REQUEST, error.to_string()),
            },
            Err(error) => Self::err(error.code(), error.to_string()),
        }
    }
}

fn params<T: DeserializeOwned>(value: Value) -> Result<T, QuickTerminalReply> {
    serde_json::from_value(value)
        .map_err(|error| QuickTerminalReply::err(INVALID_REQUEST, error.to_string()))
}

/// Execute one quick terminal request against the local service.
pub async fn dispatch(
    service: &QuickTerminalService,
    method: &str,
    value: Value,
) -> QuickTerminalReply {
    let reply = match method {
        METHOD_LIST => QuickTerminalReply::from_result(Ok::<_, QuickTerminalError>(service.list())),
        METHOD_CREATE => match params::<CreateQuickTerminal>(value) {
            Ok(request) => QuickTerminalReply::from_result(service.create(request)),
            Err(reply) => reply,
        },
        METHOD_OPEN => match params::<OpenQuickTerminal>(value) {
            Ok(request) => QuickTerminalReply::from_result(
                service
                    .open(se_quick_terminal::OpenQuickTerminal {
                        id: request.id,
                        cols: request.cols,
                        rows: request.rows,
                        terminal_id: request.terminal_id,
                    })
                    .await,
            ),
            Err(reply) => reply,
        },
        METHOD_CREATE_TERMINAL => match params::<CreateQuickTerminalTerminal>(value) {
            Ok(request) => QuickTerminalReply::from_result(
                service
                    .create_terminal(request.id, request.cols, request.rows)
                    .await,
            ),
            Err(reply) => reply,
        },
        METHOD_CLOSE_TERMINAL => match params::<CloseQuickTerminalTerminal>(value) {
            Ok(request) => QuickTerminalReply::from_result(
                service
                    .close_terminal(request.id, &request.terminal_id)
                    .await,
            ),
            Err(reply) => reply,
        },
        METHOD_RENAME => match params::<RenameQuickTerminal>(value) {
            Ok(request) => {
                QuickTerminalReply::from_result(service.rename(request.id, request.title).await)
            }
            Err(reply) => reply,
        },
        METHOD_DELETE => match params::<QuickTerminalIdParams>(value) {
            Ok(request) => QuickTerminalReply::from_result(service.delete(request.id).await),
            Err(reply) => reply,
        },
        METHOD_CLOSE => match params::<QuickTerminalIdParams>(value) {
            Ok(request) => QuickTerminalReply::from_result(service.close(request.id).await),
            Err(reply) => reply,
        },
        METHOD_IMPORT => match params::<QuickTerminalRecord>(value) {
            Ok(record) => QuickTerminalReply::from_result(service.import(record)),
            Err(reply) => reply,
        },
        other => QuickTerminalReply::err(
            INVALID_REQUEST,
            format!("unknown quick terminal method {other}"),
        ),
    };
    if let QuickTerminalReply::Err { code, .. } = &reply {
        log::warn!(
            target: "se_manager::quick_terminal",
            "operation={method} stable_code={code}"
        );
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Terminal Core only hands recognised methods to the quick terminal
    /// service; a method missing here works in process and fails in the Core.
    #[test]
    fn every_quick_terminal_method_is_routed_to_the_service() {
        for method in [
            METHOD_LIST,
            METHOD_CREATE,
            METHOD_OPEN,
            METHOD_RENAME,
            METHOD_DELETE,
            METHOD_CLOSE,
            METHOD_CREATE_TERMINAL,
            METHOD_CLOSE_TERMINAL,
            METHOD_IMPORT,
        ] {
            assert!(is_quick_terminal_method(method), "{method}");
        }
        assert!(!is_quick_terminal_method("terminalSpawn"));
    }

    #[test]
    fn terminal_tab_payloads_preserve_open_compatibility_and_reject_spawn_options() {
        let id = QuickTerminalId::new_v4();
        let legacy: OpenQuickTerminal = serde_json::from_value(serde_json::json!({
            "id": id, "cols": 80, "rows": 24
        }))
        .unwrap();
        assert_eq!(legacy.terminal_id, None);
        let selected: OpenQuickTerminal = serde_json::from_value(serde_json::json!({
            "id": id, "cols": 80, "rows": 24, "terminalId": "terminal-tab"
        }))
        .unwrap();
        assert_eq!(selected.terminal_id.as_deref(), Some("terminal-tab"));
        assert!(
            serde_json::from_value::<OpenQuickTerminal>(serde_json::json!({
                "id": id, "cols": 80, "rows": 24, "cwd": "/tmp"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<CreateQuickTerminalTerminal>(serde_json::json!({
                "id": id, "cols": 80, "rows": 24, "shell": "/bin/sh"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<CloseQuickTerminalTerminal>(serde_json::json!({
                "id": id
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn terminal_tab_dispatch_preserves_service_not_found_and_validation_errors() {
        let root = tempfile::tempdir().unwrap();
        let service = QuickTerminalService::new(
            QuickTerminalStore::open(root.path().join("quick-terminals")).unwrap(),
            root.path().join("workspace"),
            se_pty::test_pty_manager(),
        );
        let id = QuickTerminalId::new_v4();
        for (method, payload) in [
            (
                METHOD_OPEN,
                serde_json::json!({"id": id, "cols": 80, "rows": 24, "terminalId": "missing"}),
            ),
            (
                METHOD_CREATE_TERMINAL,
                serde_json::json!({"id": id, "cols": 80, "rows": 24}),
            ),
            (
                METHOD_CLOSE_TERMINAL,
                serde_json::json!({"id": id, "terminalId": "missing"}),
            ),
        ] {
            assert!(
                matches!(
                    dispatch(&service, method, payload).await,
                    QuickTerminalReply::Err {
                        code,
                        ..
                    } if code == "QUICK_TERMINAL_NOT_FOUND"
                ),
                "{method}"
            );
        }
        assert!(matches!(
            dispatch(
                &service,
                METHOD_CREATE_TERMINAL,
                serde_json::json!({
                    "id": id, "cols": 80, "rows": 24, "env": {}
                })
            )
            .await,
            QuickTerminalReply::Err {
                code,
                ..
            } if code == INVALID_REQUEST
        ));
    }
}
