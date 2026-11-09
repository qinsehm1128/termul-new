//! Host-side live terminal view shared by read-only web routes.
//!
//! In Terminal-Core mode ([`crate::core::TerminalServiceHandle::core_client`])
//! the desktop process owns no PTYs: the live terminals belong to the separate
//! `se-terminal-core` process. Reading [`crate::pty::PtyManager::get_all`] on
//! the desktop-side compatibility manager therefore reports zero terminals.
//! Every host projection that needs "what is running right now" must ask the
//! terminal service, and only fall back to the local manager when the service
//! is genuinely in-process.

use crate::web::ws::AppState;

/// One live terminal, reduced to the fields host projections need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTerminalDescriptor {
    pub id: String,
    pub pid: u32,
    pub project_id: Option<String>,
    pub cwd: String,
}

fn push_descriptor(
    out: &mut Vec<LiveTerminalDescriptor>,
    id: String,
    pid: u32,
    project_id: Option<String>,
    cwd: String,
) {
    out.push(LiveTerminalDescriptor {
        id,
        pid,
        project_id: project_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        cwd,
    });
}

/// Snapshot the terminals that are active right now.
///
/// Core mode asks Terminal Core over IPC. In-process mode reads the manager
/// that actually owns the PTYs. A failed Core read logs and reports no
/// terminals rather than inventing liveness from a local, empty manager.
pub async fn live_terminals(state: &AppState) -> Vec<LiveTerminalDescriptor> {
    let mut out = Vec::new();

    if let Some(client) = state.terminal.core_client() {
        match client.list().await {
            Ok(statuses) => {
                for status in statuses.into_iter().filter(|status| status.active) {
                    push_descriptor(
                        &mut out,
                        status.id,
                        status.pid,
                        status.project_id,
                        status.cwd,
                    );
                }
                return out;
            }
            Err(error) => {
                log::warn!(
                    target: "se_manager::web::live_terminals",
                    "operation=list_live_terminals stable_code=CORE_LIST_FAILED error={}",
                    error
                );
                return out;
            }
        }
    }

    if let Some(pty) = state.terminal.in_process_pty() {
        for instance in pty.get_all() {
            if !instance.is_active() {
                continue;
            }
            push_descriptor(
                &mut out,
                instance.id.clone(),
                instance.pid,
                instance.project_id.clone(),
                instance.cwd.clone(),
            );
        }
        return out;
    }

    // Compatibility path for focused routers/tests that pass a runtime but
    // still expose the legacy local manager on `AppState::pty`.
    for instance in state.pty.get_all() {
        if !instance.is_active() {
            continue;
        }
        push_descriptor(
            &mut out,
            instance.id.clone(),
            instance.pid,
            instance.project_id.clone(),
            instance.cwd.clone(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::pty::manager::SpawnOptions;

    #[tokio::test]
    async fn core_service_manager_is_used_instead_of_the_empty_local_tracker() {
        let local_tracker = crate::web::test_pty_manager();
        let service_pty = crate::web::test_pty_manager();
        let spawned = service_pty
            .spawn(
                SpawnOptions {
                    program: Some("/bin/cat".into()),
                    project_id: Some("project-live".into()),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("spawn test terminal");

        let state = crate::web::test_state_with_terminal(
            Arc::clone(&local_tracker),
            crate::core::TerminalServiceHandle::in_process(Arc::clone(&service_pty)),
        );

        let terminals = live_terminals(&state).await;
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].id, spawned.info.id);
        assert_eq!(terminals[0].project_id.as_deref(), Some("project-live"));

        let _ = service_pty.terminate(&spawned.info.id).await;
    }

    #[tokio::test]
    async fn no_live_service_manager_reports_no_terminals() {
        let local_tracker = crate::web::test_pty_manager();
        let state = crate::web::test_state_with_terminal(
            Arc::clone(&local_tracker),
            crate::core::TerminalServiceHandle::in_process(Arc::clone(&local_tracker)),
        );
        assert!(live_terminals(&state).await.is_empty());
    }
}
