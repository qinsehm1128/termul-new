use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;

use super::git_tracker::GitStatus;

const EVENT_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalDisplayMode {
    Phone,
    Desktop,
}

impl TerminalDisplayMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "phone" => Ok(Self::Phone),
            "desktop" => Ok(Self::Desktop),
            other => Err(format!("invalid display mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalEvent {
    Exit {
        terminal_id: String,
        exit_code: Option<i32>,
        signal: Option<i32>,
    },
    CwdChanged {
        terminal_id: String,
        cwd: String,
    },
    GitBranchChanged {
        terminal_id: String,
        branch: Option<String>,
    },
    GitStatusChanged {
        terminal_id: String,
        status: Option<GitStatus>,
    },
    ExitCodeChanged {
        terminal_id: String,
        exit_code: i32,
    },
    Spawned {
        terminal_id: String,
        project_id: Option<String>,
        conversation_id: Option<String>,
        cwd: String,
        cols: u16,
        rows: u16,
        shell: String,
    },
    DisplayModeChanged {
        terminal_id: String,
        mode: TerminalDisplayMode,
        cols: u16,
        rows: u16,
    },
}

impl TerminalEvent {
    pub fn terminal_id(&self) -> &str {
        match self {
            Self::Exit { terminal_id, .. }
            | Self::CwdChanged { terminal_id, .. }
            | Self::GitBranchChanged { terminal_id, .. }
            | Self::GitStatusChanged { terminal_id, .. }
            | Self::ExitCodeChanged { terminal_id, .. }
            | Self::Spawned { terminal_id, .. }
            | Self::DisplayModeChanged { terminal_id, .. } => terminal_id,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStateSnapshot {
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_status: Option<GitStatus>,
    pub exit_code: Option<i32>,
    pub exited: bool,
}

#[derive(Clone)]
pub struct TerminalEventHub {
    tx: Arc<broadcast::Sender<TerminalEvent>>,
    tauri: Option<AppHandle>,
    snapshots: Arc<RwLock<HashMap<String, TerminalStateSnapshot>>>,
}

impl TerminalEventHub {
    pub fn tauri(app_handle: AppHandle) -> Self {
        Self {
            tx: Arc::new(broadcast::channel(EVENT_CAPACITY).0),
            tauri: Some(app_handle),
            snapshots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn standalone() -> Self {
        Self {
            tx: Arc::new(broadcast::channel(EVENT_CAPACITY).0),
            tauri: None,
            snapshots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TerminalEvent> {
        self.tx.subscribe()
    }

    pub fn snapshot(&self, terminal_id: &str) -> TerminalStateSnapshot {
        self.snapshots
            .read()
            .get(terminal_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn remove(&self, terminal_id: &str) {
        self.snapshots.write().remove(terminal_id);
    }

    pub fn emit(&self, event: TerminalEvent) {
        {
            let mut snapshots = self.snapshots.write();
            let terminal_id = match &event {
                TerminalEvent::Exit { terminal_id, .. }
                | TerminalEvent::CwdChanged { terminal_id, .. }
                | TerminalEvent::GitBranchChanged { terminal_id, .. }
                | TerminalEvent::GitStatusChanged { terminal_id, .. }
                | TerminalEvent::ExitCodeChanged { terminal_id, .. }
                | TerminalEvent::Spawned { terminal_id, .. }
                | TerminalEvent::DisplayModeChanged { terminal_id, .. } => terminal_id.clone(),
            };
            let snapshot = snapshots.entry(terminal_id).or_default();
            match &event {
                TerminalEvent::Exit { exit_code, .. } => {
                    snapshot.exited = true;
                    snapshot.exit_code = *exit_code;
                }
                TerminalEvent::CwdChanged { cwd, .. } => snapshot.cwd = Some(cwd.clone()),
                TerminalEvent::GitBranchChanged { branch, .. } => {
                    snapshot.git_branch = branch.clone()
                }
                TerminalEvent::GitStatusChanged { status, .. } => {
                    snapshot.git_status = status.clone()
                }
                TerminalEvent::ExitCodeChanged { exit_code, .. } => {
                    snapshot.exit_code = Some(*exit_code)
                }
                TerminalEvent::Spawned { cwd, .. } => snapshot.cwd = Some(cwd.clone()),
                TerminalEvent::DisplayModeChanged { .. } => {}
            }
        }
        let _ = self.tx.send(event.clone());
        let Some(app) = &self.tauri else {
            return;
        };
        let result = match event {
            TerminalEvent::Exit {
                terminal_id,
                exit_code,
                signal,
            } => app.emit(
                "terminal-exit",
                serde_json::json!({ "id": terminal_id, "exitCode": exit_code, "signal": signal }),
            ),
            TerminalEvent::CwdChanged { terminal_id, cwd } => app.emit(
                "terminal-cwd-changed",
                serde_json::json!({ "terminalId": terminal_id, "cwd": cwd }),
            ),
            TerminalEvent::GitBranchChanged {
                terminal_id,
                branch,
            } => app.emit(
                "terminal-git-branch-changed",
                serde_json::json!({ "terminalId": terminal_id, "branch": branch }),
            ),
            TerminalEvent::GitStatusChanged {
                terminal_id,
                status,
            } => app.emit(
                "terminal-git-status-changed",
                serde_json::json!({ "terminalId": terminal_id, "status": status }),
            ),
            TerminalEvent::ExitCodeChanged {
                terminal_id,
                exit_code,
            } => app.emit(
                "terminal-exit-code-changed",
                serde_json::json!({ "terminalId": terminal_id, "exitCode": exit_code }),
            ),
            TerminalEvent::Spawned {
                terminal_id,
                project_id,
                conversation_id,
                cwd,
                cols,
                rows,
                shell,
            } => app.emit(
                "terminal-spawned",
                serde_json::json!({
                    "terminalId": terminal_id,
                    "projectId": project_id,
                    "conversationId": conversation_id,
                    "cwd": cwd,
                    "cols": cols,
                    "rows": rows,
                    "shell": shell
                }),
            ),
            TerminalEvent::DisplayModeChanged {
                terminal_id,
                mode,
                cols,
                rows,
            } => app.emit(
                "terminal-display-mode-changed",
                serde_json::json!({
                    "terminalId": terminal_id,
                    "mode": mode,
                    "cols": cols,
                    "rows": rows
                }),
            ),
        };
        if let Err(error) = result {
            log::error!("failed to emit terminal event to desktop renderer: {error}");
        }
    }
}

impl Default for TerminalEventHub {
    fn default() -> Self {
        Self::standalone()
    }
}
