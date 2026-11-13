//! Unix-socket side of a channel: connect (spawning `qin-code api-bridge` on
//! demand), perform the NDJSON hello, then hand the stream to the session.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const CONNECT_BUDGET: Duration = Duration::from_secs(5);
const RETRY_INTERVAL: Duration = Duration::from_millis(200);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Connected {
    pub writer: UnixStream,
    pub reader: BufReader<UnixStream>,
    pub hello: Value,
}

/// Primary API socket path. Mirrors `jcode_harness_api::api_socket_path`
/// (cea-jcode crates/jcode-harness-api/src/sockets.rs) so the sidecar looks
/// where `qin-code api-bridge` actually listens.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("QIN_CODE_API_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    runtime_dir().join("qin-code-api.sock")
}

/// Candidate sockets in connect order: the qin-code runtime-dir socket, then
/// the legacy `~/.qin-code/qin-code-api.sock` named in the plugin contract.
fn socket_candidates() -> Vec<PathBuf> {
    let primary = socket_path();
    let mut candidates = vec![primary.clone()];
    if std::env::var_os("QIN_CODE_API_SOCKET").is_none_or(|p| p.is_empty()) {
        if let Some(home) = dirs::home_dir() {
            let legacy = home.join(".qin-code").join("qin-code-api.sock");
            if legacy != primary {
                candidates.push(legacy);
            }
        }
    }
    candidates
}

fn runtime_dir() -> PathBuf {
    for key in [
        "QIN_CODE_RUNTIME_DIR",
        "QIN_RUNTIME_DIR",
        "QSCODE_RUNTIME_DIR",
        "JCODE_RUNTIME_DIR",
    ] {
        if let Ok(dir) = std::env::var(key) {
            if !dir.trim().is_empty() {
                return PathBuf::from(dir);
            }
        }
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "macos")]
    if let Some(dir) = darwin_user_temp_dir() {
        return dir;
    }
    #[cfg(target_os = "macos")]
    if let Ok(dir) = std::env::var("TMPDIR") {
        return PathBuf::from(dir);
    }
    std::env::temp_dir().join(format!("jcode-{}", user_discriminator()))
}

/// OS-assigned per-user temp dir; unaffected by an overridden `$TMPDIR`.
#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    // SAFETY: confstr with a null buffer only reports the required length.
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: buf has exactly `len` writable bytes.
    let written =
        unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), len) };
    if written == 0 || written > len {
        return None;
    }
    buf.truncate(written - 1);
    (!buf.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_vec(buf)))
}

fn user_discriminator() -> String {
    let raw = std::env::var("UID")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let out: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(64)
        .collect();
    if out.is_empty() {
        "user".into()
    } else {
        out
    }
}

/// `qin-code` on PATH, then `~/.local/bin/qin-code`.
fn qin_code_binary() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("qin-code"))
            .find(|candidate| candidate.is_file())
    });
    on_path.or_else(|| {
        dirs::home_dir()
            .map(|home| home.join(".local/bin/qin-code"))
            .filter(|candidate| candidate.is_file())
    })
}

/// Start `qin-code api-bridge` detached (own process group, null stdio). The
/// bridge holds a single-instance lock, so a redundant spawn exits on its own;
/// a reaper thread collects the child so an early exit leaves no zombie.
fn spawn_api_bridge() -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let binary = qin_code_binary().ok_or("qin-code binary not found (PATH, ~/.local/bin)")?;
    let mut child = Command::new(binary)
        .arg("api-bridge")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not start qin-code api-bridge: {e}"))?;
    std::thread::Builder::new()
        .name("qin-board-bridge-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn try_connect(candidates: &[PathBuf]) -> Result<UnixStream, String> {
    let mut last_error = String::from("no socket candidates");
    for path in candidates {
        match UnixStream::connect(path) {
            Ok(stream) => return Ok(stream),
            Err(e) => last_error = format!("{}: {e}", path.display()),
        }
    }
    Err(last_error)
}

fn connect() -> Result<UnixStream, String> {
    let candidates = socket_candidates();
    if let Ok(stream) = try_connect(&candidates) {
        return Ok(stream);
    }
    let spawn_error = spawn_api_bridge().err();
    let deadline = Instant::now() + CONNECT_BUDGET;
    let mut last_error = String::new();
    while Instant::now() < deadline {
        std::thread::sleep(RETRY_INTERVAL);
        match try_connect(&candidates) {
            Ok(stream) => return Ok(stream),
            Err(e) => last_error = e,
        }
    }
    Err(match spawn_error {
        Some(spawn) => format!("qin-code is not running: {spawn}"),
        None => format!("qin-code api socket unavailable: {last_error}"),
    })
}

pub fn hello_request() -> Value {
    json!({"v":1,"id":0,"req":"hello","min_version":1,"max_version":3,"client":"qin-board"})
}

/// Connect and complete the hello exchange; returns the `hello_ok` frame.
pub fn open() -> Result<Connected, String> {
    let mut writer = connect()?;
    let read_half = writer.try_clone().map_err(|e| e.to_string())?;
    read_half
        .set_read_timeout(Some(HELLO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(read_half);
    let mut line = serde_json::to_string(&hello_request()).map_err(|e| e.to_string())?;
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|e| format!("hello write failed: {e}"))?;
    for _ in 0..32 {
        let mut buf = String::new();
        let n = reader
            .read_line(&mut buf)
            .map_err(|e| format!("hello read failed: {e}"))?;
        if n == 0 {
            return Err("bridge closed during hello".into());
        }
        let Ok(frame) = serde_json::from_str::<Value>(buf.trim()) else {
            continue;
        };
        match frame["ev"].as_str() {
            Some("hello_ok") => {
                reader
                    .get_ref()
                    .set_read_timeout(None)
                    .map_err(|e| e.to_string())?;
                return Ok(Connected {
                    writer,
                    reader,
                    hello: frame,
                });
            }
            Some("error") => {
                let message = frame["message"]
                    .as_str()
                    .or_else(|| frame["error"].as_str())
                    .unwrap_or("hello rejected");
                return Err(format!("hello rejected: {message}"));
            }
            _ => {}
        }
    }
    Err("bridge did not answer hello".into())
}

/// Serialize one client frame as an NDJSON line.
pub fn write_frame(writer: &mut UnixStream, frame: &Value) -> Result<(), String> {
    let mut line = serde_json::to_string(frame).map_err(|e| e.to_string())?;
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|e| e.to_string())
}
