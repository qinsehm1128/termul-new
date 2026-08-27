//! Resolve sidecar binaries the same way `cloudflared` / `rg` are resolved.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::remote::cloudflared::configure_background_command;

static FRPC_PATH_CACHE: OnceLock<String> = OnceLock::new();

#[cfg(target_os = "windows")]
fn frpc_sidecar_name() -> &'static str {
    "frpc-x86_64-pc-windows-msvc.exe"
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn frpc_sidecar_name() -> &'static str {
    "frpc-aarch64-apple-darwin"
}
#[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
fn frpc_sidecar_name() -> &'static str {
    "frpc-x86_64-apple-darwin"
}
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn frpc_sidecar_name() -> &'static str {
    "frpc-aarch64-unknown-linux-gnu"
}
#[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
fn frpc_sidecar_name() -> &'static str {
    "frpc-x86_64-unknown-linux-musl"
}
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn frpc_sidecar_name() -> &'static str {
    "frpc"
}

pub fn resolve_sidecar_path(env_key: &str, sidecar_name: &str) -> (String, String) {
    if let Ok(env_val) = std::env::var(env_key) {
        let trimmed = env_val.trim();
        if !trimmed.is_empty() {
            let env_path = PathBuf::from(trimmed);
            if env_path.is_absolute() {
                return (trimmed.to_string(), "env".to_string());
            }
            if let Ok(cwd) = std::env::current_dir() {
                let direct = cwd.join(&env_path);
                if direct.exists() && direct.is_file() {
                    return (direct.to_string_lossy().to_string(), "env".to_string());
                }
                let from_src_tauri = cwd.join("src-tauri").join(&env_path);
                if from_src_tauri.exists() && from_src_tauri.is_file() {
                    return (
                        from_src_tauri.to_string_lossy().to_string(),
                        "env".to_string(),
                    );
                }
            }
            return (trimmed.to_string(), "env".to_string());
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("src-tauri").join("bin").join(sidecar_name));
        candidates.push(cwd.join("bin").join(sidecar_name));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join(sidecar_name));
            candidates.push(exe_dir.join("../Resources").join(sidecar_name));
            candidates.push(exe_dir.join("../lib").join(sidecar_name));
        }
    }
    if let Some(found) = candidates.into_iter().find(|p| p.exists() && p.is_file()) {
        return (found.to_string_lossy().to_string(), "sidecar".to_string());
    }
    (
        sidecar_name
            .split('-')
            .next()
            .unwrap_or(sidecar_name)
            .to_string(),
        "path".to_string(),
    )
}

pub fn detect_frpc_path() -> String {
    if let Some(cached) = FRPC_PATH_CACHE.get() {
        return cached.clone();
    }
    let (detected, _source) = resolve_sidecar_path("TERMUL_FRPC_PATH", frpc_sidecar_name());
    let _ = FRPC_PATH_CACHE.set(detected.clone());
    detected
}

pub fn configure_sidecar_command(command: &mut tokio::process::Command) {
    configure_background_command(command);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frpc_sidecar_name_is_nonempty() {
        assert!(!frpc_sidecar_name().is_empty());
    }

    #[test]
    fn resolve_path_returns_known_source() {
        let (path, source) =
            resolve_sidecar_path("TERMUL_FRPC_PATH_UNSET_FOR_TEST", frpc_sidecar_name());
        assert!(!path.is_empty());
        assert!(matches!(source.as_str(), "env" | "sidecar" | "path"));
    }
}
