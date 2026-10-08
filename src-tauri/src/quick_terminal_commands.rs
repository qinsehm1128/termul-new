//! GUI-side entry point for quick terminals: routes a request to the local
//! service or to Terminal Core and returns the renderer's `IpcResult` shape.
//! Used by the desktop Tauri commands and the `/quick-terminals` web routes.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::commands::IpcResult;
use crate::core::TerminalServiceHandle;
use crate::quick_terminal::{
    dispatch, CreateQuickTerminal, OpenQuickTerminal, QuickTerminalIdParams, QuickTerminalOpened,
    QuickTerminalRecord, QuickTerminalReply, RenameQuickTerminal, INVALID_REQUEST, METHOD_CLOSE,
    METHOD_CREATE, METHOD_DELETE, METHOD_LIST, METHOD_OPEN, METHOD_RENAME, UNAVAILABLE,
};

fn into_ipc<T: DeserializeOwned>(reply: QuickTerminalReply) -> IpcResult<T> {
    match reply {
        QuickTerminalReply::Ok { value } => match serde_json::from_value(value) {
            Ok(data) => IpcResult::success(data),
            Err(error) => IpcResult::error(error.to_string(), INVALID_REQUEST),
        },
        QuickTerminalReply::Err { code, message } => IpcResult::error(message, code),
    }
}

/// Route a request to whoever owns quick terminals for this host.
pub async fn request<P: Serialize, T: DeserializeOwned>(
    terminal: &TerminalServiceHandle,
    method: &str,
    payload: &P,
) -> IpcResult<T> {
    let value = match serde_json::to_value(payload) {
        Ok(value) => value,
        Err(error) => return IpcResult::error(error.to_string(), INVALID_REQUEST),
    };
    let reply = if let Some(service) = terminal.quick_terminals() {
        dispatch(&service, method, value).await
    } else if let Some(client) = terminal.core_client() {
        match client.request(method, value).await {
            Ok(raw) => serde_json::from_value(raw).unwrap_or_else(|error| {
                QuickTerminalReply::err(INVALID_REQUEST, error.to_string())
            }),
            Err(error) => QuickTerminalReply::err(UNAVAILABLE, error.command_message()),
        }
    } else {
        QuickTerminalReply::err(
            UNAVAILABLE,
            "quick terminals are not available on this host",
        )
    };
    into_ipc(reply)
}

#[tauri::command]
pub async fn quick_terminal_list(
    terminal: State<'_, TerminalServiceHandle>,
) -> Result<IpcResult<Vec<QuickTerminalRecord>>, String> {
    Ok(request(terminal.inner(), METHOD_LIST, &Value::Null).await)
}

#[tauri::command]
pub async fn quick_terminal_create(
    terminal: State<'_, TerminalServiceHandle>,
    payload: CreateQuickTerminal,
) -> Result<IpcResult<QuickTerminalRecord>, String> {
    Ok(request(terminal.inner(), METHOD_CREATE, &payload).await)
}

#[tauri::command]
pub async fn quick_terminal_open(
    terminal: State<'_, TerminalServiceHandle>,
    payload: OpenQuickTerminal,
) -> Result<IpcResult<QuickTerminalOpened>, String> {
    Ok(request(terminal.inner(), METHOD_OPEN, &payload).await)
}

#[tauri::command]
pub async fn quick_terminal_rename(
    terminal: State<'_, TerminalServiceHandle>,
    payload: RenameQuickTerminal,
) -> Result<IpcResult<QuickTerminalRecord>, String> {
    Ok(request(terminal.inner(), METHOD_RENAME, &payload).await)
}

#[tauri::command]
pub async fn quick_terminal_delete(
    terminal: State<'_, TerminalServiceHandle>,
    payload: QuickTerminalIdParams,
) -> Result<IpcResult<()>, String> {
    Ok(request(terminal.inner(), METHOD_DELETE, &payload).await)
}

#[tauri::command]
pub async fn quick_terminal_close(
    terminal: State<'_, TerminalServiceHandle>,
    payload: QuickTerminalIdParams,
) -> Result<IpcResult<QuickTerminalRecord>, String> {
    Ok(request(terminal.inner(), METHOD_CLOSE, &payload).await)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::quick_terminal::{with_local_service, QuickTerminalId};

    #[tokio::test]
    async fn an_in_process_host_answers_like_terminal_core() {
        // Durable directory creation refuses symlinked components (macOS `/var`).
        let profile_dir = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let profile = profile_dir.path().canonicalize().unwrap();
        let workspace = workspace_dir.path().canonicalize().unwrap();
        let handle = with_local_service(
            TerminalServiceHandle::in_process(crate::pty::test_pty_manager()),
            &profile,
            &workspace,
        );
        assert!(handle.quick_terminals().is_some());

        let created: IpcResult<QuickTerminalRecord> = request(
            &handle,
            METHOD_CREATE,
            &CreateQuickTerminal {
                target: se_quick_terminal::QuickTerminalTarget::Workspace,
                title: None,
            },
        )
        .await;
        let record = created.data.expect("created in process");
        let renamed: IpcResult<QuickTerminalRecord> = request(
            &handle,
            METHOD_RENAME,
            &RenameQuickTerminal {
                id: record.id,
                title: Some("renamed".to_string()),
            },
        )
        .await;
        assert_eq!(
            renamed.data.and_then(|record| record.title).as_deref(),
            Some("renamed")
        );

        let closed: IpcResult<QuickTerminalRecord> = request(
            &handle,
            METHOD_CLOSE,
            &QuickTerminalIdParams { id: record.id },
        )
        .await;
        let closed = closed.data.expect("closed in process");
        assert_eq!((closed.id, closed.terminal_id), (record.id, None));

        let bad: IpcResult<QuickTerminalRecord> =
            request(&handle, METHOD_RENAME, &serde_json::json!({ "id": "nope" })).await;
        assert_eq!(bad.code.as_deref(), Some(INVALID_REQUEST));
        let gone: IpcResult<()> = request(
            &handle,
            METHOD_DELETE,
            &QuickTerminalIdParams {
                id: QuickTerminalId::new_v4(),
            },
        )
        .await;
        assert_eq!(gone.code.as_deref(), Some("QUICK_TERMINAL_NOT_FOUND"));
    }

    #[tokio::test]
    async fn a_host_without_quick_terminals_says_so() {
        let handle = TerminalServiceHandle::in_process(crate::pty::test_pty_manager());
        let listed: IpcResult<Vec<QuickTerminalRecord>> =
            request(&handle, METHOD_LIST, &Value::Null).await;
        assert_eq!(listed.code.as_deref(), Some(UNAVAILABLE));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_core_serves_quick_terminals_with_their_error_codes() {
        use crate::core::{CoreEndpoint, CoreRole, TerminalCoreClient};
        use crate::quick_terminal::{
            CreateQuickTerminal, OpenQuickTerminal, QuickTerminalIdParams, QuickTerminalOpened,
            QuickTerminalRecord, METHOD_CREATE, METHOD_DELETE, METHOD_LIST, METHOD_OPEN,
        };
        // Durable directory creation refuses symlinked components (macOS `/var`).
        let profile_dir = tempfile::tempdir().unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let profile = profile_dir.path().canonicalize().unwrap();
        let workspace = workspace_dir.path().canonicalize().unwrap();
        let endpoint = CoreEndpoint::for_profile(&profile, CoreRole::TerminalCore);
        let server_endpoint = endpoint.clone();
        let roots = (profile.clone(), workspace.clone());
        let program = crate::terminal_program();
        let server = tokio::spawn(async move {
            crate::core::terminal::run_terminal_core_with(server_endpoint, Some(roots), program)
                .await
        });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let client = loop {
            match TerminalCoreClient::connect(&endpoint).await {
                Ok(client) => break client,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(error) => panic!("terminal core did not become ready: {error}"),
            }
        };
        let handle = TerminalServiceHandle::from_core_client(client);
        assert!(
            handle.quick_terminals().is_none(),
            "Core mode owns no local store"
        );

        let created: crate::commands::IpcResult<QuickTerminalRecord> = request(
            &handle,
            METHOD_CREATE,
            &CreateQuickTerminal {
                target: se_quick_terminal::QuickTerminalTarget::Workspace,
                title: Some("core".to_string()),
            },
        )
        .await;
        let record = created.data.expect("created through Terminal Core");
        assert!(record
            .cwd
            .starts_with(workspace.join("terminals").to_str().unwrap()));

        let opened: crate::commands::IpcResult<QuickTerminalOpened> = request(
            &handle,
            METHOD_OPEN,
            &OpenQuickTerminal {
                id: record.id,
                cols: 80,
                rows: 24,
            },
        )
        .await;
        let opened = opened.data.expect("opened through Terminal Core");
        assert!(opened.spawned && opened.claim.is_some());

        let missing: crate::commands::IpcResult<QuickTerminalOpened> = request(
            &handle,
            METHOD_OPEN,
            &OpenQuickTerminal {
                id: crate::quick_terminal::QuickTerminalId::new_v4(),
                cols: 80,
                rows: 24,
            },
        )
        .await;
        assert!(!missing.success);
        assert_eq!(missing.code.as_deref(), Some("QUICK_TERMINAL_NOT_FOUND"));

        let deleted: crate::commands::IpcResult<()> = request(
            &handle,
            METHOD_DELETE,
            &QuickTerminalIdParams { id: record.id },
        )
        .await;
        assert!(deleted.success);
        let listed: crate::commands::IpcResult<Vec<QuickTerminalRecord>> =
            request(&handle, METHOD_LIST, &serde_json::Value::Null).await;
        assert_eq!(listed.data.map(|records| records.len()), Some(0));
        server.abort();
    }
}
