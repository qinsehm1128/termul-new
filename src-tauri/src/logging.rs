//! Production logging & observability (issue #244).
//!
//! Installs a persistent, rotated file sink in release builds via
//! `tauri-plugin-log`, captures Rust panics with a backtrace, logs a startup
//! banner, and exposes a per-run session id used to correlate user-attached
//! log slices with a single run.

use std::panic;
use std::sync::OnceLock;

use log::LevelFilter;
use tauri::{Manager, Runtime};
use tauri_plugin_log::{Builder as LogBuilder, Target, TargetKind};
use uuid::Uuid;

/// Maximum size of a single log file before it is rotated (5 MB). Old logs are
/// renamed to a timestamped file (`KeepAll`) so the lifecycle narrative that
/// led to a crash survives rotation while individual files stay attachable.
const MAX_LOG_FILE_SIZE: u128 = 5 * 1024 * 1024;

/// Base file name (without extension) for the Rust log in the OS log dir.
const LOG_FILE_NAME: &str = "termul";

static SESSION_ID: OnceLock<String> = OnceLock::new();

/// Short, per-process correlation id. Generated once on first access and
/// included in the startup banner so a user-attached log slice can be tied to
/// a single run.
pub fn session_id() -> &'static str {
    SESSION_ID.get_or_init(|| Uuid::new_v4().simple().to_string()[..8].to_string())
}

/// Build channel string for the startup banner.
pub fn build_channel() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Parsed `RUST_LOG` directives: a global threshold plus any per-module
/// overrides. Kept separate so the per-module scoping survives instead of being
/// flattened to one global level.
pub struct LogDirectives {
    pub global: LevelFilter,
    pub per_module: Vec<(String, LevelFilter)>,
}

/// Default global level when `RUST_LOG` names no bare level: `info` in release,
/// `debug` in debug builds.
fn default_floor() -> LevelFilter {
    if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    }
}

/// Third-party crates that dump huge payloads at debug during a normal run.
/// Applied before `RUST_LOG` so an explicit override still wins.
fn default_quiet_modules() -> Vec<(String, LevelFilter)> {
    vec![
        // The plugin logs the full updater JSON, including the entire release
        // notes markdown, at DEBUG on every check.
        (
            "tauri_plugin_updater".to_string(),
            LevelFilter::Info,
        ),
    ]
}

fn parse_level(token: &str) -> Option<LevelFilter> {
    match token.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(LevelFilter::Trace),
        "debug" => Some(LevelFilter::Debug),
        "info" => Some(LevelFilter::Info),
        "warn" => Some(LevelFilter::Warn),
        "error" => Some(LevelFilter::Error),
        "off" => Some(LevelFilter::Off),
        _ => None,
    }
}

/// Resolve `RUST_LOG` into a global level plus per-module overrides.
///
/// `tauri-plugin-log` uses `fern` and does not parse `RUST_LOG` itself, so we
/// parse it to preserve the documented override behavior:
/// - `RUST_LOG=trace` → global trace.
/// - `RUST_LOG=off` → global off (logging genuinely disabled).
/// - `RUST_LOG=termul_manager_lib=debug` → only that module at debug; everything
///   else stays at the floor (no third-party crate flooding).
/// - `RUST_LOG=hyper=warn,termul_manager_lib=trace` → each module scoped
///   independently.
///
/// When `RUST_LOG` is unset or names no bare level, the global stays at the
/// floor (`info`/`debug`). Unrecognized tokens are ignored.
pub fn resolve_directives() -> LogDirectives {
    match std::env::var("RUST_LOG") {
        Ok(value) if !value.trim().is_empty() => parse_directives(&value),
        _ => LogDirectives {
            global: default_floor(),
            per_module: Vec::new(),
        },
    }
}

fn parse_directives(spec: &str) -> LogDirectives {
    let mut global: Option<LevelFilter> = None;
    let mut per_module: Vec<(String, LevelFilter)> = Vec::new();

    for part in spec.split([',', ' ', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('=') {
            // `module=level` — scoped override (ignored if level unrecognized).
            Some((module, level_str)) => {
                let module = module.trim();
                if let (false, Some(level)) = (module.is_empty(), parse_level(level_str)) {
                    per_module.push((module.to_string(), level));
                }
            }
            // Bare token: a level sets the global threshold; a bare module name
            // (no level) is ignored rather than silently widening verbosity.
            None => {
                if let Some(level) = parse_level(part) {
                    global = Some(level);
                }
            }
        }
    }

    LogDirectives {
        global: global.unwrap_or_else(default_floor),
        per_module,
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
        file_name: Some(LOG_FILE_NAME.to_string()),
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

/// Install a global panic hook that routes panic payloads + a captured
/// backtrace to the `log` facade, so panics land in the file sink instead of a
/// discarded stderr. Chains to the previously installed hook.
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
                log::info!(target: "termul::tracing", "{trimmed}");
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

pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());

        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());

        let backtrace = std::backtrace::Backtrace::force_capture();

        log::error!(
            "[PANIC] [session {}] thread '{}' panicked at {}: {}\n{}",
            session_id(),
            std::thread::current().name().unwrap_or("<unnamed>"),
            location,
            message,
            backtrace
        );

        // Preserve default behavior (prints to stderr in debug, aborts flow).
        previous(info);
    }));
}

/// Resolve the absolute path of the active log file (`<app_log_dir>/termul.log`).
/// The `LogDir` target writes `{file_name}.log`, so we append the `.log`
/// extension the plugin adds.
pub fn log_file_path<R: Runtime>(app: &tauri::AppHandle<R>) -> Option<std::path::PathBuf> {
    app.path()
        .app_log_dir()
        .ok()
        .map(|dir| dir.join(format!("{}.log", LOG_FILE_NAME)))
}

/// Emit a single startup banner at `info` level: version, OS/arch, build
/// channel, session id, and the resolved absolute log file path. Lets a
/// maintainer reading a log file know exactly what produced it.
pub fn log_startup_banner<R: Runtime>(app: &tauri::AppHandle<R>) {
    let log_file = log_file_path(app)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unavailable>".to_string());

    log::info!(
        "[startup] termul v{} | {} {} | channel={} | session={} | log={}",
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

    const BRIDGE_CHILD_CASE: &str = "TERMUL_LOGGING_BRIDGE_CHILD_CASE";

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

    #[test]
    fn session_id_is_stable_and_short() {
        let a = session_id();
        let b = session_id();
        assert_eq!(a, b, "session id must be stable within a process");
        assert_eq!(a.len(), 8, "session id is the 8-char short form");
    }

    #[test]
    fn default_quiet_modules_keep_updater_payloads_off_debug() {
        assert!(
            default_quiet_modules()
                .iter()
                .any(|(module, level)| module == "tauri_plugin_updater" && *level == LevelFilter::Info),
            "updater changelog dumps must stay at info unless RUST_LOG overrides"
        );
    }

    #[test]
    fn bare_level_sets_global_no_module_overrides() {
        let d = parse_directives("trace");
        assert_eq!(d.global, LevelFilter::Trace);
        assert!(d.per_module.is_empty());

        let d = parse_directives("warn");
        assert_eq!(d.global, LevelFilter::Warn);
    }

    #[test]
    fn off_genuinely_disables_logging() {
        let d = parse_directives("off");
        assert_eq!(d.global, LevelFilter::Off);
        assert!(d.per_module.is_empty());
    }

    #[test]
    fn module_scoped_directive_keeps_global_at_floor() {
        // `termul_manager=debug` must NOT raise other crates: global stays at
        // the floor, the module gets its own override.
        let d = parse_directives("termul_manager=debug");
        assert_eq!(d.global, default_floor());
        assert_eq!(
            d.per_module,
            vec![("termul_manager".to_string(), LevelFilter::Debug)]
        );
    }

    #[test]
    fn multiple_modules_are_scoped_independently() {
        let d = parse_directives("hyper=warn,termul_manager=trace");
        assert_eq!(d.global, default_floor());
        assert_eq!(
            d.per_module,
            vec![
                ("hyper".to_string(), LevelFilter::Warn),
                ("termul_manager".to_string(), LevelFilter::Trace),
            ]
        );
    }

    #[test]
    fn bare_module_name_without_level_is_ignored() {
        // A bare module name (no level) must not silently widen verbosity.
        let d = parse_directives("some_module");
        assert_eq!(d.global, default_floor());
        assert!(d.per_module.is_empty());
    }

    #[test]
    fn global_level_and_module_override_combine() {
        let d = parse_directives("info,termul_manager=trace");
        assert_eq!(d.global, LevelFilter::Info);
        assert_eq!(
            d.per_module,
            vec![("termul_manager".to_string(), LevelFilter::Trace)]
        );
    }

    #[test]
    fn unrecognized_level_tokens_are_ignored() {
        let d = parse_directives("bogus,termul_manager=nonsense");
        assert_eq!(d.global, default_floor());
        assert!(d.per_module.is_empty());
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
        tracing::info!(target: "termul::web::ws", stable_code = "OK", "WebSocket upgrade Origin accepted");
        let captured = captured_messages("termul::tracing");
        assert!(
            captured.iter().any(|message| {
                message.contains("termul::web::ws")
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
        tracing::info!(target: "termul::web::ws", "bridge-before-capture");
        let captured = captured_messages("termul::tracing");
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
            tracing::info!(target: "termul::web::ws", stable_code = "OK", "scoped bridge audit");
            log::info!(target: "termul::web::auth", "scoped auth capture");

            let bridge = crate::web::auth::test_tracing::messages_for(scope_id, "termul::tracing");
            assert!(
                bridge
                    .iter()
                    .any(|message| message.contains("scoped bridge audit")),
                "scoped capture must receive tracing bridge output: {bridge:?}"
            );
            let auth = crate::web::auth::test_tracing::messages_for(scope_id, "termul::web::auth");
            assert!(
                auth.iter()
                    .any(|message| message.contains("scoped auth capture")),
                "scoped auth capture must remain usable after bridge setup: {auth:?}"
            );
        });
    }
}
