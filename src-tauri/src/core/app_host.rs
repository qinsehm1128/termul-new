//! macOS: start a Core as an app of its own instead of as a child of the GUI.
//!
//! A Core spawned by the GUI belongs to the GUI app's coalition, and `setsid`
//! does not change that. When the GUI quits while a Core is still running, the
//! Dock marks Se Manager as running in the background, and its "Stop Running
//! in Background" item force-quits the whole coalition. A launchd job that the
//! GUI submits is still attributed to the GUI and is stopped too. Every
//! terminal and agent went with it.
//!
//! An app launched through LaunchServices gets its own coalition. So each Core
//! is launched as a tiny faceless host app (`LSUIElement`). The host lives
//! under the profile root, and its executable is a symlink to the real Core
//! binary inside Se Manager.app. The kernel resolves the symlink, so the Core
//! still runs the bundled, signed binary and finds its siblings beside it.
//! `open` passes the caller's environment to the app, so the Core still gets
//! the GUI's environment, as it did when it was a child.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The `.app` that contains `executable`, when it runs from
/// `<name>.app/Contents/MacOS/`. `None` for a bare binary (`cargo run`,
/// tests), which keeps the plain child spawn.
pub(crate) fn containing_app(executable: &Path) -> Option<&Path> {
    let macos_dir = executable.parent()?;
    let contents = macos_dir.parent()?;
    let app = contents.parent()?;
    let is_bundle = macos_dir.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app";
    is_bundle.then_some(app)
}

/// A process that runs in a host app: the suffix of the host's bundle id
/// (`<Se Manager id>.<id_suffix>`) and the label in its name.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HostedProcess {
    pub(crate) id_suffix: &'static str,
    pub(crate) label: &'static str,
}

pub(crate) const TERMINAL_CORE: HostedProcess = HostedProcess {
    id_suffix: "terminal-core",
    label: "Terminal Core",
};
pub(crate) const ACP_CORE: HostedProcess = HostedProcess {
    id_suffix: "acp-core",
    label: "Agent Core",
};
pub(crate) const MCP_GATEWAY: HostedProcess = HostedProcess {
    id_suffix: "mcp-gateway",
    label: "MCP Gateway",
};

/// Identity of the host app for one [`HostedProcess`].
pub(crate) struct HostApp {
    pub(crate) bundle: PathBuf,
    pub(crate) bundle_id: String,
    pub(crate) name: String,
}

impl HostApp {
    pub(crate) fn new(
        profile_root: &Path,
        app: &Path,
        app_bundle_id: &str,
        process: HostedProcess,
    ) -> Self {
        let app_name = app
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Se Manager".to_string());
        let name = format!("{app_name} {}", process.label);
        Self {
            bundle: profile_root.join("core-hosts").join(format!("{name}.app")),
            bundle_id: format!("{app_bundle_id}.{}", process.id_suffix),
            name,
        }
    }

    fn info_plist(&self, executable_name: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>{}</string>
	<key>CFBundleName</key>
	<string>{}</string>
	<key>CFBundleExecutable</key>
	<string>{}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>LSUIElement</key>
	<true/>
</dict>
</plist>
"#,
            xml_escape(&self.bundle_id),
            xml_escape(&self.name),
            xml_escape(executable_name),
        )
    }

    /// Create or repair the host bundle so it runs `executable`. Files that
    /// are already right are left alone, so LaunchServices sees the same
    /// bundle on every launch.
    pub(crate) fn ensure(&self, executable: &Path) -> std::io::Result<()> {
        let executable_name = executable
            .file_name()
            .ok_or_else(|| std::io::Error::other("core executable has no file name"))?
            .to_string_lossy()
            .into_owned();
        let macos_dir = self.bundle.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos_dir)?;

        let plist_path = self.bundle.join("Contents").join("Info.plist");
        let plist = self.info_plist(&executable_name);
        if std::fs::read_to_string(&plist_path).ok().as_deref() != Some(plist.as_str()) {
            std::fs::write(&plist_path, plist)?;
        }

        let link = macos_dir.join(&executable_name);
        if std::fs::read_link(&link).ok().as_deref() != Some(executable) {
            match std::fs::remove_file(&link) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            std::os::unix::fs::symlink(executable, &link)?;
        }
        Ok(())
    }

    /// `open` arguments that start a new, hidden instance of the host with
    /// `env` added and `args` passed to the process.
    pub(crate) fn open_args(
        &self,
        args: &[&OsStr],
        env: &[(&str, &Path)],
        stderr: Option<&Path>,
    ) -> Vec<OsString> {
        let mut open: Vec<OsString> = vec!["-n".into(), "-g".into(), "-j".into()];
        for (key, value) in env {
            let mut pair = OsString::from(format!("{key}="));
            pair.push(value);
            open.push("--env".into());
            open.push(pair);
        }
        if let Some(stderr) = stderr {
            open.push("--stderr".into());
            open.push(stderr.into());
        }
        open.push(self.bundle.clone().into());
        open.push("--args".into());
        open.extend(args.iter().map(|arg| arg.to_os_string()));
        open
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn main_bundle_identifier() -> Option<String> {
    use objc2_foundation::NSBundle;
    NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|id| id.to_string())
}

/// Launch `executable args` in its own host app through LaunchServices.
/// Returns `false` when the GUI is not running from an app bundle or the
/// launch failed; the caller then spawns the process as a child, as before.
pub(crate) fn launch(
    process: HostedProcess,
    executable: &Path,
    args: &[&OsStr],
    env: &[(&str, &Path)],
    stderr: Option<&Path>,
    profile_root: &Path,
) -> bool {
    let Some(app) = containing_app(executable) else {
        return false;
    };
    let Some(app_bundle_id) = main_bundle_identifier() else {
        return false;
    };
    let host = HostApp::new(profile_root, app, &app_bundle_id, process);
    if let Err(error) = host.ensure(executable) {
        log::warn!(
            target: "se_manager::core",
            "operation=core_host_prepare process={} stable_code=CORE_HOST_UNAVAILABLE error={error}",
            process.id_suffix
        );
        return false;
    }

    match Command::new("/usr/bin/open")
        .args(host.open_args(args, env, stderr))
        .output()
    {
        Ok(output) if output.status.success() => {
            log::info!(
                target: "se_manager::core",
                "operation=core_host_launch process={} stable_code=LAUNCHED host={}",
                process.id_suffix,
                host.bundle.display()
            );
            true
        }
        Ok(output) => {
            log::warn!(
                target: "se_manager::core",
                "operation=core_host_launch process={} stable_code=CORE_HOST_LAUNCH_FAILED status={} stderr={}",
                process.id_suffix,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            false
        }
        Err(error) => {
            log::warn!(
                target: "se_manager::core",
                "operation=core_host_launch process={} stable_code=CORE_HOST_LAUNCH_FAILED error={error}",
                process.id_suffix
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_executable_inside_an_app_bundle_has_a_containing_app() {
        assert_eq!(
            containing_app(Path::new(
                "/Applications/Se Manager.app/Contents/MacOS/se-terminal-core"
            )),
            Some(Path::new("/Applications/Se Manager.app"))
        );
        assert_eq!(
            containing_app(Path::new("/repo/target/debug/se-manager")),
            None
        );
        assert_eq!(
            containing_app(Path::new("/x/Se.bundle/Contents/MacOS/se")),
            None
        );
        assert_eq!(
            containing_app(Path::new("/x/Se.app/Contents/Resources/se")),
            None
        );
    }

    #[test]
    fn each_role_gets_its_own_faceless_host_under_the_profile() {
        let profile = Path::new("/profile");
        let app = Path::new("/Applications/Se Manager.app");
        let terminal = HostApp::new(profile, app, "com.se-manager.app", TERMINAL_CORE);
        let acp = HostApp::new(profile, app, "com.se-manager.app", ACP_CORE);
        let gateway = HostApp::new(profile, app, "com.se-manager.app", MCP_GATEWAY);

        assert_eq!(
            terminal.bundle,
            Path::new("/profile/core-hosts/Se Manager Terminal Core.app")
        );
        assert_eq!(terminal.bundle_id, "com.se-manager.app.terminal-core");
        assert_eq!(acp.bundle_id, "com.se-manager.app.acp-core");
        assert_eq!(gateway.bundle_id, "com.se-manager.app.mcp-gateway");
        assert_ne!(terminal.bundle, acp.bundle);
        assert_ne!(acp.bundle, gateway.bundle);

        let plist = terminal.info_plist("se-terminal-core");
        assert!(plist.contains("<key>LSUIElement</key>\n\t<true/>"));
        assert!(plist.contains("<string>com.se-manager.app.terminal-core</string>"));
        assert!(
            plist.contains("<key>CFBundleExecutable</key>\n\t<string>se-terminal-core</string>")
        );
    }

    #[test]
    fn ensure_links_the_core_binary_and_follows_a_moved_app() {
        let dir = tempfile::tempdir().unwrap();
        let host = HostApp::new(
            dir.path(),
            Path::new("/Applications/Se Manager.app"),
            "com.se-manager.app",
            TERMINAL_CORE,
        );
        let first = dir.path().join("a/se-terminal-core");
        let moved = dir.path().join("b/se-terminal-core");
        let link = host.bundle.join("Contents/MacOS/se-terminal-core");

        host.ensure(&first).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), first);
        let plist = host.bundle.join("Contents/Info.plist");
        assert_eq!(
            std::fs::read_to_string(&plist).unwrap(),
            host.info_plist("se-terminal-core")
        );

        host.ensure(&first).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), first);

        host.ensure(&moved).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), moved);
    }

    #[test]
    fn open_starts_a_new_hidden_instance_with_the_core_environment() {
        let host = HostApp::new(
            Path::new("/p"),
            Path::new("/Applications/Se Manager.app"),
            "com.se-manager.app",
            MCP_GATEWAY,
        );
        let args = host.open_args(
            &[
                OsStr::new("--mcp-core"),
                OsStr::new("--config"),
                OsStr::new("/p/gw.json"),
            ],
            &[("TERMUL_CORE_PROFILE_ROOT", Path::new("/p"))],
            Some(Path::new("/logs/core.log")),
        );
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-n",
                "-g",
                "-j",
                "--env",
                "TERMUL_CORE_PROFILE_ROOT=/p",
                "--stderr",
                "/logs/core.log",
                "/p/core-hosts/Se Manager MCP Gateway.app",
                "--args",
                "--mcp-core",
                "--config",
                "/p/gw.json",
            ]
        );
    }
}
