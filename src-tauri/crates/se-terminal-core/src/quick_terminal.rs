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
    CreateQuickTerminal, OpenQuickTerminal, QuickTerminalError, QuickTerminalId,
    QuickTerminalOpened, QuickTerminalRecord, QuickTerminalService, QuickTerminalStore,
};

pub const METHOD_LIST: &str = "quickTerminalList";
pub const METHOD_CREATE: &str = "quickTerminalCreate";
pub const METHOD_OPEN: &str = "quickTerminalOpen";
pub const METHOD_RENAME: &str = "quickTerminalRename";
pub const METHOD_DELETE: &str = "quickTerminalDelete";
pub const METHOD_CLOSE: &str = "quickTerminalClose";
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
            Ok(request) => QuickTerminalReply::from_result(service.open(request).await),
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
            METHOD_IMPORT,
        ] {
            assert!(is_quick_terminal_method(method), "{method}");
        }
        assert!(!is_quick_terminal_method("terminalSpawn"));
    }
}
