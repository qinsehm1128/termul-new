//! Shared sidecar spawn + readiness wait.

use std::time::Duration;

use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

use super::sidecar::configure_sidecar_command;
use crate::remote::cloudflared::configure_background_command;

pub fn spawn_sidecar(
    path: &str,
    args: &[&str],
    extra_env: &[(&str, String)],
) -> Result<Child, String> {
    let mut command = Command::new(path);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    crate::remote::cloudflared::strip_proxy_env(&mut command);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    configure_sidecar_command(&mut command);
    configure_background_command(&mut command);
    command.spawn().map_err(|e| {
        log::error!(
            target: "se_manager::remote::tunnel",
            "operation=tunnel_sidecar_spawn stable_code=SPAWN_FAILED path_kind={}",
            if path.contains(std::path::MAIN_SEPARATOR) {
                "resolved"
            } else {
                "bare"
            }
        );
        format!("tunnel sidecar failed to spawn at {path}: {e}")
    })
}

/// Wait until `ready_re` matches stdout/stderr, the child exits, or `timeout` elapses.
///
/// A still-running child after the deadline is treated as ready when
/// `allow_timeout_if_alive` is true (used when the public Origin is known a
/// priori and log format varies across sidecar versions).
pub async fn wait_for_ready(
    child: &mut Child,
    timeout: Duration,
    ready_re: &Regex,
    allow_timeout_if_alive: bool,
) -> Result<(), String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "tunnel sidecar stdout pipe missing".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "tunnel sidecar stderr pipe missing".to_string())?;

    let (ready_tx, ready_rx) = oneshot::channel::<()>();
    let ready_tx = std::sync::Arc::new(Mutex::new(Some(ready_tx)));
    spawn_pattern_scanner(stdout, ready_re.clone(), ready_tx.clone());
    spawn_pattern_scanner(stderr, ready_re.clone(), ready_tx);

    tokio::select! {
        biased;
        status = child.wait() => {
            match status {
                Ok(code) => Err(format!("tunnel sidecar exited before becoming ready ({code})")),
                Err(e) => Err(format!("tunnel sidecar wait failed: {e}")),
            }
        }
        ready = ready_rx => {
            match ready {
                Ok(()) => Ok(()),
                Err(_) => Err("tunnel sidecar closed logs before becoming ready".to_string()),
            }
        }
        _ = tokio::time::sleep(timeout) => {
            if allow_timeout_if_alive {
                log::warn!(
                    target: "se_manager::remote::tunnel",
                    "operation=tunnel_sidecar_ready stable_code=READY_TIMEOUT_ALIVE"
                );
                Ok(())
            } else {
                Err(format!(
                    "tunnel sidecar did not become ready within {}s",
                    timeout.as_secs()
                ))
            }
        }
    }
}

fn spawn_pattern_scanner<R>(
    reader: R,
    ready_re: Regex,
    ready_tx: std::sync::Arc<Mutex<Option<oneshot::Sender<()>>>>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    if ready_re.is_match(&line) {
                        if let Some(tx) = ready_tx.lock().await.take() {
                            let _ = tx.send(());
                        }
                        // Keep draining so the sidecar does not get SIGPIPE.
                    }
                }
                Err(_) => break,
            }
        }
        ready_tx.lock().await.take();
    });
}
