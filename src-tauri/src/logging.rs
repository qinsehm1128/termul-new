//! Production logging & observability (issue #244).
//!
//! Installs a persistent, rotated file sink in release builds via
//! `tauri-plugin-log`, captures Rust panics with a backtrace, logs a startup
//! banner, and exposes a per-run session id used to correlate user-attached
//! log slices with a single run.

use tauri::{Manager, Runtime};
use tauri_plugin_log::{Builder as LogBuilder, Target, TargetKind};

use crate::brand;
pub use se_foundation::logging::{
    default_quiet_modules, install_core_logger, install_panic_hook, open_core_log_file,
    resolve_directives, session_id, CORE_LOG_FILE_ENV, MAX_LOG_FILE_SIZE,
};

/// Base file name (without extension) for the Rust log in the OS log dir.
///
/// The name is not a literal here: `.github/ISSUE_TEMPLATE/bug_report.yml`
/// publishes the resulting absolute path on all three platforms, and that
/// template is prose nothing compiles. `scripts/tests/log-path-parity.test.ts`
/// holds the two sides together by deriving the published path from
/// [`brand::canonical`]'s `log_file_name` plus the bundle identifier — which
/// only works while this function is the sole place the name is read.
///
/// Reads the brand seam, so it must be called on the thread that owns it
/// (FORBID-07). Both callers do.
fn log_file_name() -> &'static str {
    brand::canonical().log_file_name
}

/// Build channel string for the startup banner.
pub fn build_channel() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Build the `tauri-plugin-log` plugin.
///
/// - Debug builds: log to stdout (developer console) plus the OS log dir.
/// - Release builds: log to the OS log dir only (no console exists on Windows
///   release; stderr is discarded).
///
/// The default plugin format already prefixes every line with timestamp,
/// level, and target, satisfying the structured-line requirement.
pub fn build_log_plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let mut targets = vec![Target::new(TargetKind::LogDir {
        file_name: Some(log_file_name().to_string()),
    })];

    if cfg!(debug_assertions) {
        targets.push(Target::new(TargetKind::Stdout));
    }

    let directives = resolve_directives();

    let mut builder = LogBuilder::new()
        .targets(targets)
        .level(directives.global)
        // KeepOne caps disk usage: on rotation the previous file is discarded
        // rather than retained forever, so a chatty or crash-looping release
        // build cannot grow the log directory without bound.
        .max_file_size(MAX_LOG_FILE_SIZE)
        .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne);

    // Quiet known-noisy crates first; RUST_LOG overrides still win after.
    for (module, level) in default_quiet_modules() {
        builder = builder.level_for(module, level);
    }
    // Apply per-module RUST_LOG overrides so scoping survives instead of
    // flattening to one global level.
    for (module, level) in directives.per_module {
        builder = builder.level_for(module, level);
    }

    builder.build()
}

/// Bridge `tracing` events from shared web/WS admission paths into the `log`
/// facade so Desktop Tauri captures Origin/admission/lifecycle audits.
pub fn install_desktop_tracing_bridge() {
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(true)
        .with_writer(TracingToLogWriter)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

struct TracingToLogWriter;

impl std::io::Write for TracingToLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        thread_local! {
            static EMITTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        if EMITTING.with(|flag| flag.replace(true)) {
            return Ok(buf.len());
        }
        if let Ok(line) = std::str::from_utf8(buf) {
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                log::info!(target: "se_manager::tracing", "{trimmed}");
            }
        }
        EMITTING.with(|flag| flag.set(false));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TracingToLogWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        TracingToLogWriter
    }
}

/// Stem shared by the Core log files, `<log_file_name>-<role>.log`, so they sit
/// next to — and sort with — the GUI log in the same directory.
///
/// Reads the brand seam, so it must be called on the caller's own thread
/// (FORBID-07).
#[must_use]
pub fn core_log_file_prefix() -> &'static str {
    log_file_name()
}

/// The active log file's on-disk name, `<log_file_name>.log`.
///
/// Shared with the "save a copy of the log" flows in `lib.rs`: those offer a
/// default file name in a save dialog, and a literal there would drift from the
/// file being copied the moment the brand flipped — the copy would still be the
/// right bytes under a name that no longer matches anything the app writes.
///
/// Reads the brand seam, so it must be called on the caller's own thread
/// (FORBID-07).
#[must_use]
pub fn log_file_base_name() -> String {
    format!("{}.log", log_file_name())
}

/// Resolve the absolute path of the active log file
/// (`<app_log_dir>/<log_file_name>.log`). The `LogDir` target writes
/// `{file_name}.log`, so we append the `.log` extension the plugin adds.
pub fn log_file_path<R: Runtime>(app: &tauri::AppHandle<R>) -> Option<std::path::PathBuf> {
    app.path()
        .app_log_dir()
        .ok()
        .map(|dir| dir.join(log_file_base_name()))
}

/// Emit a single startup banner at `info` level: version, OS/arch, build
/// channel, session id, and the resolved absolute log file path. Lets a
/// maintainer reading a log file know exactly what produced it.
pub fn log_startup_banner<R: Runtime>(app: &tauri::AppHandle<R>) {
    let log_file = log_file_path(app)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unavailable>".to_string());

    log::info!(
        "[startup] se-manager v{} | {} {} | channel={} | session={} | log={}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        build_channel(),
        session_id(),
        log_file
    );
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::process::Command;
    use std::sync::Mutex;

    use super::*;

    const BRIDGE_CHILD_CASE: &str = "SE_LOGGING_BRIDGE_CHILD_CASE";

    #[derive(Clone)]
    struct CapturedLog {
        target: String,
        message: String,
    }

    struct BridgeCaptureLogger {
        records: Mutex<Vec<CapturedLog>>,
    }

    impl log::Log for BridgeCaptureLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            self.records.lock().unwrap().push(CapturedLog {
                target: record.target().to_string(),
                message: record.args().to_string(),
            });
        }

        fn flush(&self) {}
    }

    static BRIDGE_CAPTURE_LOGGER: BridgeCaptureLogger = BridgeCaptureLogger {
        records: Mutex::new(Vec::new()),
    };

    fn run_in_isolated_test_process(case: &str, test_name: &str) -> bool {
        if std::env::var_os(BRIDGE_CHILD_CASE).as_deref() == Some(OsStr::new(case)) {
            return true;
        }

        let output = Command::new(std::env::current_exe().unwrap())
            .env(BRIDGE_CHILD_CASE, case)
            .arg(test_name)
            .arg("--exact")
            .arg("--nocapture")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated logging test {case} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        false
    }

    fn install_bridge_capture_logger() {
        log::set_logger(&BRIDGE_CAPTURE_LOGGER)
            .expect("desktop tracing bridge must leave the global log logger unclaimed");
        log::set_max_level(log::LevelFilter::Trace);
    }

    fn captured_messages(target: &str) -> Vec<String> {
        BRIDGE_CAPTURE_LOGGER
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.target == target)
            .map(|record| record.message.clone())
            .collect()
    }

    /// The published log paths in `.github/ISSUE_TEMPLATE/bug_report.yml` are
    /// derived from `brand::canonical().log_file_name`. That derivation is only
    /// true of the shipped binary while this module reads the seam instead of
    /// carrying its own copy of the name.
    #[test]
    fn log_file_name_follows_the_brand_seam_rather_than_a_literal() {
        assert_eq!(log_file_name(), brand::DEFAULT_CANONICAL.log_file_name);

        // The injected name has to be one `canonical()` never returns on its
        // own, or the assertion below passes without the seam being consulted.
        // Now that T-A17 has flipped the contract, that is the legacy spelling.
        let _guard = brand::override_canonical(brand::BrandCanonical {
            log_file_name: brand::LEGACY.log_file_name,
            ..brand::DEFAULT_CANONICAL
        });
        assert_ne!(
            brand::LEGACY.log_file_name,
            brand::DEFAULT_CANONICAL.log_file_name,
            "the injected name must differ from the shipped one or this proves nothing"
        );
        assert_eq!(
            log_file_name(),
            brand::LEGACY.log_file_name,
            "a rename of brand::canonical().log_file_name must move the log file the app writes; \
             a literal here would strand every path bug_report.yml publishes"
        );
    }

    /// `lib.rs`'s two "save a copy of the log" dialogs offer this as the
    /// default file name; a literal in either would name a file the app has
    /// stopped writing.
    #[test]
    fn the_saved_copy_is_offered_under_the_name_the_app_actually_writes() {
        let _guard = brand::override_canonical(brand::BrandCanonical {
            log_file_name: brand::LEGACY.log_file_name,
            ..brand::DEFAULT_CANONICAL
        });
        assert_eq!(
            log_file_base_name(),
            format!("{}.log", brand::LEGACY.log_file_name)
        );
    }

    #[test]
    fn build_channel_matches_compilation_profile() {
        let expected = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        assert_eq!(build_channel(), expected);
    }

    #[test]
    fn desktop_tracing_bridge_captures_ws_audit_events() {
        if !run_in_isolated_test_process(
            "capture-ws-audit",
            "logging::tests::desktop_tracing_bridge_captures_ws_audit_events",
        ) {
            return;
        }

        install_bridge_capture_logger();
        install_desktop_tracing_bridge();
        tracing::info!(target: "se_manager::web::ws", stable_code = "OK", "WebSocket upgrade Origin accepted");
        let captured = captured_messages("se_manager::tracing");
        assert!(
            captured.iter().any(|message| {
                message.contains("se_manager::web::ws")
                    && message.contains("stable_code=\"OK\"")
                    && message.contains("WebSocket upgrade Origin accepted")
            }),
            "WS audit tracing event must reach the desktop log facade: {captured:?}"
        );
    }

    #[test]
    fn desktop_tracing_bridge_leaves_global_log_logger_unclaimed() {
        if !run_in_isolated_test_process(
            "bridge-before-log-capture",
            "logging::tests::desktop_tracing_bridge_leaves_global_log_logger_unclaimed",
        ) {
            return;
        }

        install_desktop_tracing_bridge();
        install_bridge_capture_logger();
        tracing::info!(target: "se_manager::web::ws", "bridge-before-capture");
        let captured = captured_messages("se_manager::tracing");
        assert!(
            captured
                .iter()
                .any(|message| message.contains("bridge-before-capture")),
            "bridge installed before the log capture must still forward: {captured:?}"
        );
    }

    #[test]
    fn scoped_auth_capture_logger_coexists_after_bridge_setup() {
        if !run_in_isolated_test_process(
            "bridge-before-auth-capture",
            "logging::tests::scoped_auth_capture_logger_coexists_after_bridge_setup",
        ) {
            return;
        }

        install_desktop_tracing_bridge();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let scoped = crate::web::auth::test_tracing::lock_scoped("task-005-bridge-order").await;
            let scope_id = scoped.id();
            tracing::info!(target: "se_manager::web::ws", stable_code = "OK", "scoped bridge audit");
            log::info!(target: "se_manager::web::auth", "scoped auth capture");

            let bridge = crate::web::auth::test_tracing::messages_for(scope_id, "se_manager::tracing");
            assert!(
                bridge
                    .iter()
                    .any(|message| message.contains("scoped bridge audit")),
                "scoped capture must receive tracing bridge output: {bridge:?}"
            );
            let auth = crate::web::auth::test_tracing::messages_for(scope_id, "se_manager::web::auth");
            assert!(
                auth.iter()
                    .any(|message| message.contains("scoped auth capture")),
                "scoped auth capture must remain usable after bridge setup: {auth:?}"
            );
        });
    }
}
