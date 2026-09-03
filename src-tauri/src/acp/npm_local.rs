//! Materialize `npx -y <package>` ACP agents into a host-owned local Node prefix.
//!
//! First launch runs `npm install --prefix <root>/<slug> <package>` once. Later
//! launches skip npm and start `node <local bin>` directly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use crate::acp::config::AgentConfig;

const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);

static ROOT: OnceLock<PathBuf> = OnceLock::new();
/// Default on: first `npx -y` spawn installs into a host prefix, later spawns
/// start the local bin. App Preferences can turn this off to always use npx.
/// `TERMUL_ACP_PREFER_LOCAL_NPM` (0/1/true/false) wins when set.
static PREFER_LOCAL_NPM: AtomicBool = AtomicBool::new(true);

/// Push the App Preferences toggle into the spawn path.
pub fn set_prefer_local_npm_install(prefer: bool) {
    PREFER_LOCAL_NPM.store(prefer, Ordering::SeqCst);
}

fn prefer_local_npm_install() -> bool {
    match std::env::var("TERMUL_ACP_PREFER_LOCAL_NPM") {
        Ok(value) if value == "0" || value.eq_ignore_ascii_case("false") => false,
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => true,
        _ => PREFER_LOCAL_NPM.load(Ordering::SeqCst),
    }
}

/// Pin the host-owned npm prefix. Desktop uses
/// `<app_data_dir>/acp-npm-packages`; standalone uses
/// `<state dir>/acp-npm-packages`. Tests may override via `TERMUL_ACP_NPM_ROOT`.
pub fn set_root(path: PathBuf) {
    let _ = ROOT.set(path);
}

fn root() -> PathBuf {
    if let Ok(override_root) = std::env::var("TERMUL_ACP_NPM_ROOT") {
        if !override_root.is_empty() {
            return PathBuf::from(override_root);
        }
    }
    ROOT.get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("se-manager-acp-npm-packages"))
}

/// Rewrite an `npx -y <package>` config to a locally installed Node entrypoint
/// when possible. Non-npx configs are returned unchanged. A failed install
/// falls back to the original npx command.
pub fn materialize_npx_config(mut config: AgentConfig) -> AgentConfig {
    if !prefer_local_npm_install() {
        log::info!("[acp-npm] preferring npx launch (local install disabled)");
        return config;
    }
    let Some((package, extra_args)) = parse_npx_invocation(&config.command, &config.args) else {
        return config;
    };
    let Some(slug) = package_slug(&package) else {
        return config;
    };
    let prefix = root().join(slug);
    if let Some((command, args)) = resolve_local_launch(&prefix, &package, &extra_args) {
        log::info!(
            "[acp-npm] using local install package={} command={}",
            package,
            command
        );
        config.command = command;
        config.args = args;
        return config;
    }
    match install_package(&prefix, &package) {
        Ok(()) => {
            if let Some((command, args)) = resolve_local_launch(&prefix, &package, &extra_args) {
                log::info!(
                    "[acp-npm] installed local package={} command={}",
                    package,
                    command
                );
                config.command = command;
                config.args = args;
            } else {
                log::warn!(
                    "[acp-npm] install finished but local bin missing package={}",
                    package
                );
            }
        }
        Err(error) => {
            log::warn!(
                "[acp-npm] local install failed package={} detail={error}; falling back to npx",
                package
            );
        }
    }
    config
}

fn parse_npx_invocation(command: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    let basename = command.replace('\\', "/");
    let basename = basename.rsplit('/').next().unwrap_or(command);
    if !matches!(basename, "npx" | "npx.cmd" | "npx.exe") {
        return None;
    }
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if matches!(arg, "-y" | "--yes" | "--") {
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return None;
        }
        if !is_safe_package(arg) {
            return None;
        }
        return Some((arg.to_string(), args[index + 1..].to_vec()));
    }
    None
}

fn is_safe_package(spec: &str) -> bool {
    if spec.is_empty() || spec.starts_with('-') || spec.contains("..") || spec.contains('\\') {
        return false;
    }
    spec.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '@' | '/' | '.' | '-' | '_' | '+'))
}

fn package_slug(spec: &str) -> Option<String> {
    if !is_safe_package(spec) {
        return None;
    }
    let slug = spec.replace(['@', '/'], "-").trim_matches('-').to_string();
    if slug.is_empty() {
        return None;
    }
    Some(slug)
}

fn package_name_without_version(spec: &str) -> &str {
    if let Some(rest) = spec.strip_prefix('@') {
        if let Some(slash) = rest.find('/') {
            let after_name = &rest[slash + 1..];
            if let Some(at) = after_name.find('@') {
                return &spec[..2 + slash + at];
            }
        }
        return spec;
    }
    spec.split_once('@').map_or(spec, |(name, _)| name)
}

fn resolve_local_launch(
    prefix: &Path,
    spec: &str,
    extra_args: &[String],
) -> Option<(String, Vec<String>)> {
    // npm's `.bin` shim is the most reliable launcher across JS, shell, and
    // platform wrappers (Codex/Gemini/Claude all land here after install).
    if let Some(shim) = resolve_local_shim(prefix, spec) {
        return Some((shim.to_string_lossy().into_owned(), extra_args.to_vec()));
    }
    let bin = resolve_local_bin(prefix, spec)?;
    let node = resolve_node()?;
    let mut args = vec![bin.to_string_lossy().into_owned()];
    args.extend(extra_args.iter().cloned());
    Some((node, args))
}

fn resolve_local_shim(prefix: &Path, spec: &str) -> Option<PathBuf> {
    let name = package_name_without_version(spec);
    let short = name.rsplit('/').next().unwrap_or(name);
    if short.is_empty() || short.contains("..") {
        return None;
    }
    let bin_dir = prefix.join("node_modules").join(".bin");
    let unix = bin_dir.join(short);
    if unix.is_file() {
        return Some(unix);
    }
    if cfg!(windows) {
        for ext in ["cmd", "exe", "bat"] {
            let with_ext = bin_dir.join(format!("{short}.{ext}"));
            if with_ext.is_file() {
                return Some(with_ext);
            }
        }
    }
    None
}

fn resolve_local_bin(prefix: &Path, spec: &str) -> Option<PathBuf> {
    let name = package_name_without_version(spec);
    let pkg_dir = prefix.join("node_modules").join(name);
    let manifest = std::fs::read_to_string(pkg_dir.join("package.json")).ok()?;
    let json: Value = serde_json::from_str(&manifest).ok()?;
    let rel = match json.get("bin")? {
        Value::String(path) => path.clone(),
        Value::Object(map) => {
            let short = name.rsplit('/').next().unwrap_or(name);
            map.get(short)
                .or_else(|| map.values().next())
                .and_then(Value::as_str)?
                .to_string()
        }
        _ => return None,
    };
    if rel.is_empty() || rel.contains("..") {
        return None;
    }
    let path = pkg_dir.join(rel);
    path.is_file().then_some(path)
}

fn resolve_node() -> Option<String> {
    match crate::pty::manager::resolve_spawn_program("node") {
        Ok(resolved) => Some(resolved.program),
        Err(_) => {
            let mut env_map = HashMap::new();
            crate::pty::env_refresh::apply_fresh_path(&mut env_map);
            let path = env_map.get("PATH")?;
            resolve_on_path("node", path)
        }
    }
}

fn resolve_on_path(command: &str, path: &str) -> Option<String> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    for dir in path.split(separator).filter(|segment| !segment.is_empty()) {
        let candidate = Path::new(dir).join(command);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
        if cfg!(windows) {
            for ext in ["exe", "cmd", "bat"] {
                let with_ext = Path::new(dir).join(format!("{command}.{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

fn install_package(prefix: &Path, spec: &str) -> Result<(), String> {
    std::fs::create_dir_all(prefix)
        .map_err(|error| format!("create npm prefix failed: {error}"))?;
    let npm = crate::trackers::git_tracker::resolve_executable("npm");
    let mut env_map = HashMap::new();
    crate::pty::env_refresh::apply_fresh_path(&mut env_map);
    let mut command = std::process::Command::new(npm);
    command
        .args([
            "install",
            "--omit=dev",
            "--no-fund",
            "--no-audit",
            "--no-update-notifier",
            "--prefix",
        ])
        .arg(prefix)
        .arg(spec)
        .stdin(std::process::Stdio::null());
    if let Some(path) = env_map.get("PATH") {
        command.env("PATH", path);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn npm failed: {error}"))?;
    match wait_with_timeout(&mut child, INSTALL_TIMEOUT) {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("npm install exited {status}")),
        Err(error) => {
            let _ = child.kill();
            Err(error)
        }
    }
}

fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() >= timeout => {
                return Err("npm install timed out".to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(format!("wait npm failed: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn config(command: &str, args: &[&str]) -> AgentConfig {
        AgentConfig {
            config_id: Some("cfg-1".to_string()),
            name: "Claude Agent".to_string(),
            command: command.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            env: HashMap::new(),
            allow_terminal: false,
            permission_policy: crate::acp::config::PermissionPolicy::Ask,
        }
    }

    #[test]
    fn parse_npx_yes_package_and_passthrough_args() {
        let parsed = parse_npx_invocation(
            "npx",
            &[
                "-y".to_string(),
                "@agentclientprotocol/claude-agent-acp".to_string(),
                "--acp".to_string(),
            ],
        );
        assert_eq!(
            parsed,
            Some((
                "@agentclientprotocol/claude-agent-acp".to_string(),
                vec!["--acp".to_string()]
            ))
        );
    }

    #[test]
    fn parse_rejects_flag_injection_and_non_npx() {
        assert!(parse_npx_invocation("uvx", &["pkg".to_string()]).is_none());
        assert!(parse_npx_invocation("npx", &["--package=evil".to_string()]).is_none());
        assert!(parse_npx_invocation("npx", &["-y".to_string(), "--evil".to_string()]).is_none());
        assert!(parse_npx_invocation("npx", &["-y".to_string(), "../evil".to_string()]).is_none());
    }

    #[test]
    fn package_name_strips_version_for_scoped_and_plain() {
        assert_eq!(
            package_name_without_version("@scope/name@1.2.3"),
            "@scope/name"
        );
        assert_eq!(package_name_without_version("plain@2.0.0"), "plain");
        assert_eq!(package_name_without_version("@scope/name"), "@scope/name");
    }

    #[test]
    fn resolve_local_bin_reads_package_json_bin() {
        let temp = tempfile::tempdir().unwrap();
        let pkg = temp
            .path()
            .join("node_modules")
            .join("@agentclientprotocol")
            .join("claude-agent-acp");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::create_dir_all(pkg.join("dist")).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            r#"{"bin":{"claude-agent-acp":"dist/index.js"}}"#,
        )
        .unwrap();
        std::fs::write(pkg.join("dist").join("index.js"), "console.log(1)\n").unwrap();
        let bin = resolve_local_bin(temp.path(), "@agentclientprotocol/claude-agent-acp@0.1.0");
        assert_eq!(bin, Some(pkg.join("dist").join("index.js")));
    }

    #[test]
    fn materialize_leaves_non_npx_unchanged() {
        let original = config("uvx", &["some-package"]);
        let next = materialize_npx_config(original);
        assert_eq!(next.command, "uvx");
        assert_eq!(next.args, vec!["some-package".to_string()]);
    }

    #[test]
    fn materialize_rewrites_npx_when_local_bin_exists() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("TERMUL_ACP_NPM_ROOT", temp.path());
        let slug = package_slug("@agentclientprotocol/claude-agent-acp").unwrap();
        let pkg = temp
            .path()
            .join(slug)
            .join("node_modules")
            .join("@agentclientprotocol")
            .join("claude-agent-acp");
        std::fs::create_dir_all(pkg.join("dist")).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            r#"{"bin":{"claude-agent-acp":"dist/index.js"}}"#,
        )
        .unwrap();
        std::fs::write(pkg.join("dist").join("index.js"), "console.log(1)\n").unwrap();

        let next = materialize_npx_config(config(
            "npx",
            &["-y", "@agentclientprotocol/claude-agent-acp", "--acp"],
        ));
        std::env::remove_var("TERMUL_ACP_NPM_ROOT");
        if resolve_node().is_none() {
            assert_eq!(next.command, "npx");
            return;
        }
        assert_ne!(next.command, "npx");
        assert!(
            next.args[0].ends_with("dist/index.js") || next.args[0].ends_with("dist\\index.js")
        );
        assert_eq!(next.args.last().map(String::as_str), Some("--acp"));
    }

    #[test]
    fn materialize_uses_npm_bin_shim_when_present() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        std::env::remove_var("TERMUL_ACP_PREFER_LOCAL_NPM");
        set_prefer_local_npm_install(true);
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("TERMUL_ACP_NPM_ROOT", temp.path());
        let slug = package_slug("@zed-industries/codex-acp").unwrap();
        let bin_dir = temp.path().join(slug).join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let shim = bin_dir.join("codex-acp");
        std::fs::write(&shim, "#!/bin/sh\n").unwrap();

        let next = materialize_npx_config(config("npx", &["-y", "@zed-industries/codex-acp"]));
        std::env::remove_var("TERMUL_ACP_NPM_ROOT");
        assert_eq!(next.command, shim.to_string_lossy());
        assert!(next.args.is_empty());
    }

    #[test]
    fn materialize_keeps_npx_when_local_install_disabled() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        std::env::remove_var("TERMUL_ACP_PREFER_LOCAL_NPM");
        set_prefer_local_npm_install(false);
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("TERMUL_ACP_NPM_ROOT", temp.path());
        let slug = package_slug("@zed-industries/codex-acp").unwrap();
        let bin_dir = temp.path().join(slug).join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("codex-acp"), "#!/bin/sh\n").unwrap();

        let next = materialize_npx_config(config("npx", &["-y", "@zed-industries/codex-acp"]));
        set_prefer_local_npm_install(true);
        std::env::remove_var("TERMUL_ACP_NPM_ROOT");
        assert_eq!(next.command, "npx");
        assert_eq!(
            next.args,
            vec!["-y".to_string(), "@zed-industries/codex-acp".to_string()]
        );
    }

    #[test]
    fn parse_accepts_bundled_npx_agent_packages() {
        for package in [
            "@google/gemini-cli",
            "@agentclientprotocol/claude-agent-acp",
            "@zed-industries/codex-acp",
            "@github/copilot",
            "@qwen-code/qwen-code",
            "cline",
            "@augmentcode/auggie",
        ] {
            assert!(
                parse_npx_invocation("npx", &["-y".to_string(), package.to_string()]).is_some(),
                "{package}"
            );
        }
    }
}
