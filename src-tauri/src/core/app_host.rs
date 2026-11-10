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
//! under the profile root and runs a byte-identical copy of the real Core
//! binary (see [`HostApp::ensure`] for the payload). `open` passes the
//! caller's environment to the app, so the Core still gets the GUI's
//! environment, as it did when it was a child.
//!
//! The host must NOT symlink (or hardlink) the executable inside
//! `Se Manager.app/Contents/MacOS/`. LaunchServices decides whether an app is
//! running by looking for a live process on that app's `CFBundleExecutable`
//! vnode, not by bundle id: with such a link, a surviving host process (the
//! MCP gateway is meant to outlive the GUI) keeps LaunchServices convinced the
//! GUI is still running, so `open "/Applications/Se Manager.app"` returns 0
//! and starts nothing until the host exits. A copy is a distinct vnode, keeps
//! the code signature valid, and leaves `open` working.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
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
pub(crate) const PLUGIN_CORE: HostedProcess = HostedProcess {
    id_suffix: "plugin-core",
    label: "Plugin Core",
};
pub(crate) const MCP_GATEWAY: HostedProcess = HostedProcess {
    id_suffix: "mcp-gateway",
    label: "MCP Gateway",
};
pub(crate) const MCP_AGGREGATOR: HostedProcess = HostedProcess {
    id_suffix: "mcp-gateway-aggregator",
    label: "MCP Aggregator",
};

/// What [`HostApp::ensure`] did with the executable payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PayloadState {
    /// The host already ran a copy of the current Core binary.
    Reused,
    /// The payload was (re)written from the current Core binary.
    Refreshed,
    /// The copy failed, so the host runs the Core through a link to the app
    /// binary. The Core still launches, but LaunchServices counts it as the
    /// GUI while it lives.
    Unavailable,
}

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

    /// Create or repair the host bundle so it runs a copy of `executable`.
    ///
    /// The host must not link the app's own executable: LaunchServices treats
    /// a live process on the app's `CFBundleExecutable` vnode as "the app is
    /// running", so a host that outlives the GUI (the MCP gateway does by
    /// design) would make `open Se Manager.app` a silent no-op. The payload is
    /// copied once per profile and shared by every host of that file name; it
    /// is refreshed when the app binary changes (length, mtime, or version).
    /// Running hosts keep their old inode until they exit, so a refresh never
    /// truncates a live core.
    pub(crate) fn ensure(&self, executable: &Path) -> io::Result<PayloadState> {
        let executable_name = executable
            .file_name()
            .ok_or_else(|| io::Error::other("core executable has no file name"))?
            .to_string_lossy()
            .into_owned();
        let macos_dir = self.bundle.join("Contents").join("MacOS");
        fs::create_dir_all(&macos_dir)?;

        let plist_path = self.bundle.join("Contents").join("Info.plist");
        let plist = self.info_plist(&executable_name);
        if fs::read_to_string(&plist_path).ok().as_deref() != Some(plist.as_str()) {
            fs::write(&plist_path, plist)?;
        }

        let link = macos_dir.join(&executable_name);
        match self.sync_payload(executable, &executable_name) {
            Ok((payload, state)) => {
                point_at(&link, &payload)?;
                Ok(state)
            }
            Err(error) => {
                // Never make the core unlaunchable because the copy failed.
                // The legacy link keeps it running, at the cost of the
                // relaunch failure this module is here to prevent.
                log::warn!(
                    target: "se_manager::core",
                    "operation=core_host_payload process={} stable_code=PAYLOAD_UNAVAILABLE kind={:?}",
                    self.name,
                    error.kind()
                );
                point_at(&link, executable)?;
                Ok(PayloadState::Unavailable)
            }
        }
    }

    /// Copy `executable` into the profile's shared payload when the existing
    /// copy no longer matches it. Returns the payload path and what happened.
    fn sync_payload(
        &self,
        executable: &Path,
        executable_name: &str,
    ) -> io::Result<(PathBuf, PayloadState)> {
        let payload_dir = self
            .bundle
            .parent()
            .ok_or_else(|| io::Error::other("core host bundle has no profile root"))?
            .join("payload");
        fs::create_dir_all(&payload_dir)?;
        let payload = payload_dir.join(executable_name);
        let stamp = payload_dir.join(format!("{executable_name}.stamp"));
        let fingerprint = source_fingerprint(executable)?;

        let copied = fs::symlink_metadata(&payload).is_ok_and(|metadata| metadata.is_file());
        if copied && fs::read_to_string(&stamp).ok().as_deref() == Some(fingerprint.as_str()) {
            return Ok((payload, PayloadState::Reused));
        }

        // Copy beside the payload and rename over it: readers see either the
        // old inode or the complete new one, never a half-written binary.
        let temp = payload_dir.join(format!(
            ".{executable_name}.{}.tmp",
            std::process::id()
        ));
        let result = fs::copy(executable, &temp)
            .and_then(|_| fs::rename(&temp, &payload))
            .and_then(|_| write_if_changed(&stamp, &fingerprint));
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        Ok((payload, PayloadState::Refreshed))
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

/// Point `link` at `target`, repairing a legacy link to the app binary.
fn point_at(link: &Path, target: &Path) -> io::Result<()> {
    if fs::read_link(link).ok().as_deref() == Some(target) {
        return Ok(());
    }
    match fs::remove_file(link) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    std::os::unix::fs::symlink(target, link)
}

/// Identity of the Core binary on disk. A replaced or rebuilt app changes the
/// length or mtime (or the crate version), which is enough to refresh the
/// payload without hashing the whole binary on every launch.
fn source_fingerprint(executable: &Path) -> io::Result<String> {
    let metadata = fs::metadata(executable)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| format!("{}.{:09}", elapsed.as_secs(), elapsed.subsec_nanos()))
        .unwrap_or_else(|| "unknown".to_string());
    Ok(format!(
        "host_payload=1\nversion={}\nlength={}\nmodified={modified}\n",
        env!("CARGO_PKG_VERSION"),
        metadata.len()
    ))
}

fn write_if_changed(path: &Path, contents: &str) -> io::Result<()> {
    if fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    let temp = path.with_extension("stamp.tmp");
    fs::write(&temp, contents)?;
    fs::rename(&temp, path)
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
    match host.ensure(executable) {
        Ok(PayloadState::Reused) => {}
        Ok(PayloadState::Refreshed) => log::info!(
            target: "se_manager::core",
            "operation=core_host_payload process={} stable_code=REFRESHED",
            process.id_suffix
        ),
        Ok(PayloadState::Unavailable) => log::warn!(
            target: "se_manager::core",
            "operation=core_host_payload process={} stable_code=UNAVAILABLE",
            process.id_suffix
        ),
        Err(error) => {
            log::warn!(
                target: "se_manager::core",
                "operation=core_host_prepare process={} stable_code=CORE_HOST_UNAVAILABLE kind={:?}",
                process.id_suffix,
                error.kind()
            );
            return false;
        }
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

    fn write_binary(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn payload_of(host: &HostApp, executable_name: &str) -> PathBuf {
        host.bundle
            .parent()
            .unwrap()
            .join("payload")
            .join(executable_name)
    }

    /// The regression this module exists for: a host that runs the app's own
    /// executable keeps LaunchServices convinced the GUI is still running, so
    /// `open Se Manager.app` starts nothing. The host must run its own copy.
    #[test]
    fn ensure_copies_the_binary_instead_of_linking_the_app_executable() {
        let dir = tempfile::tempdir().unwrap();
        let host = HostApp::new(
            dir.path(),
            Path::new("/Applications/Se Manager.app"),
            "com.se-manager.app",
            MCP_GATEWAY,
        );
        let app_binary = dir.path().join("app/Contents/MacOS/se-manager");
        write_binary(&app_binary, b"core-binary");

        assert_eq!(host.ensure(&app_binary).unwrap(), PayloadState::Refreshed);

        let link = host.bundle.join("Contents/MacOS/se-manager");
        let payload = payload_of(&host, "se-manager");
        assert_eq!(std::fs::read_link(&link).unwrap(), payload);
        assert_eq!(std::fs::read(&payload).unwrap(), b"core-binary");
        assert!(std::fs::symlink_metadata(&payload).unwrap().is_file());
        assert_eq!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(&payload).unwrap()
        );
        assert_ne!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(&app_binary).unwrap(),
            "a host must never resolve to the app executable's vnode"
        );
        let plist = host.bundle.join("Contents/Info.plist");
        assert_eq!(
            std::fs::read_to_string(&plist).unwrap(),
            host.info_plist("se-manager")
        );
    }

    #[test]
    fn ensure_reuses_an_unchanged_payload_and_refreshes_a_replaced_binary() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let host = HostApp::new(
            dir.path(),
            Path::new("/Applications/Se Manager.app"),
            "com.se-manager.app",
            MCP_GATEWAY,
        );
        let app_binary = dir.path().join("app/Contents/MacOS/se-manager");
        write_binary(&app_binary, b"first");

        host.ensure(&app_binary).unwrap();
        let payload = payload_of(&host, "se-manager");
        let first_inode = std::fs::metadata(&payload).unwrap().ino();

        assert_eq!(host.ensure(&app_binary).unwrap(), PayloadState::Reused);
        assert_eq!(
            std::fs::metadata(&payload).unwrap().ino(),
            first_inode,
            "an unchanged binary must not be copied again"
        );

        write_binary(&app_binary, b"second-build");
        assert_eq!(host.ensure(&app_binary).unwrap(), PayloadState::Refreshed);
        assert_eq!(std::fs::read(&payload).unwrap(), b"second-build");
        assert_ne!(std::fs::metadata(&payload).unwrap().ino(), first_inode);
    }

    /// Upgrading from a build that linked the app binary must repair the host
    /// instead of leaving the relaunch block in place.
    #[test]
    fn ensure_replaces_a_legacy_link_to_the_app_executable() {
        let dir = tempfile::tempdir().unwrap();
        let host = HostApp::new(
            dir.path(),
            Path::new("/Applications/Se Manager.app"),
            "com.se-manager.app",
            PLUGIN_CORE,
        );
        let app_binary = dir.path().join("app/Contents/MacOS/se-manager");
        write_binary(&app_binary, b"core-binary");
        let link = host.bundle.join("Contents/MacOS/se-manager");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&app_binary, &link).unwrap();

        assert_eq!(host.ensure(&app_binary).unwrap(), PayloadState::Refreshed);
        assert_eq!(std::fs::read_link(&link).unwrap(), payload_of(&host, "se-manager"));
        assert_ne!(std::fs::read_link(&link).unwrap(), app_binary);
    }

    #[test]
    fn plugin_core_has_a_distinct_host_and_main_binary_flag() {
        let host = HostApp::new(
            Path::new("/p"),
            Path::new("/Applications/Se Manager.app"),
            "com.se-manager.app",
            PLUGIN_CORE,
        );
        let plist = host.info_plist("se-manager");
        assert!(plist.contains("com.se-manager.app.plugin-core"));
        let args = host.open_args(
            &[OsStr::new("--plugin-core")],
            &[("TERMUL_CORE_PROFILE_ROOT", Path::new("/p"))],
            None,
        );
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"/p/core-hosts/Se Manager Plugin Core.app".to_string()));
        assert!(args.contains(&"TERMUL_CORE_PROFILE_ROOT=/p".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("--plugin-core"));
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
