//! Logging pieces shared by the GUI and the headless Core processes:
//! `RUST_LOG` parsing, the panic hook, the per-run session id, and the Core
//! file logger. The GUI's `tauri-plugin-log` setup lives in the app crate.

use std::panic;
use std::sync::OnceLock;

use log::LevelFilter;
use uuid::Uuid;

/// Maximum size of a single log file before it is rotated (5 MB). Old logs are
/// renamed to a timestamped file (`KeepAll`) so the lifecycle narrative that
/// led to a crash survives rotation while individual files stay attachable.
pub const MAX_LOG_FILE_SIZE: u128 = 5 * 1024 * 1024;

static SESSION_ID: OnceLock<String> = OnceLock::new();

/// Short, per-process correlation id. Generated once on first access and
/// included in the startup banner so a user-attached log slice can be tied to
/// a single run.
pub fn session_id() -> &'static str {
    SESSION_ID.get_or_init(|| Uuid::new_v4().simple().to_string()[..8].to_string())
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
pub fn default_quiet_modules() -> Vec<(String, LevelFilter)> {
    vec![
        // The plugin logs the full updater JSON, including the entire release
        // notes markdown, at DEBUG on every check.
        ("tauri_plugin_updater".to_string(), LevelFilter::Info),
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
/// - `RUST_LOG=se_manager_lib=debug` → only that module at debug; everything
///   else stays at the floor (no third-party crate flooding).
/// - `RUST_LOG=hyper=warn,se_manager_lib=trace` → each module scoped
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

/// Install a global panic hook that routes panic payloads + a captured
/// backtrace to the `log` facade, so panics land in the file sink instead of a
/// discarded stderr. Chains to the previously installed hook.
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

/// Environment variable carrying a Core process's log file path. The GUI
/// resolves the path (brand seam, log dir) and the headless Core only opens it.
pub const CORE_LOG_FILE_ENV: &str = "TERMUL_CORE_LOG_FILE";

/// Open a Core log file for appending, creating its directory. A file that
/// has already outgrown [`MAX_LOG_FILE_SIZE`] is first renamed to
/// `<stem>.old.log`, replacing the previous one (the GUI's `KeepOne` policy).
///
/// The launcher hands the same file to the Core as its stderr, so panics and
/// `eprintln!` output land beside the `log` records.
pub fn open_core_log_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(metadata) = std::fs::metadata(path) {
        if u128::from(metadata.len()) > MAX_LOG_FILE_SIZE {
            let _ = std::fs::rename(path, path.with_extension("old.log"));
        }
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

/// Route a headless Core process's `log` records to the file named by
/// [`CORE_LOG_FILE_ENV`] and install the panic hook. Without this a Core has no
/// logger at all and its stderr is discarded, so why it exited is invisible.
///
/// Returns the log path, or `None` when the variable is unset (a Core started
/// by hand) or the file cannot be opened.
pub fn install_core_logger() -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from(std::env::var_os(CORE_LOG_FILE_ENV)?);
    let file = open_core_log_file(&path).ok()?;
    let logger: &'static CoreFileLogger =
        Box::leak(Box::new(CoreFileLogger::new(file, resolve_directives())));
    log::set_logger(logger).ok()?;
    log::set_max_level(logger.max_level());
    install_panic_hook();
    Some(path)
}

/// `log` backend for Core processes. Lines match the GUI file format
/// (`[date][time][target][LEVEL] message`, UTC) so both files read alike.
struct CoreFileLogger {
    file: std::sync::Mutex<std::fs::File>,
    global: LevelFilter,
    /// Quiet defaults first, then `RUST_LOG` overrides; the last match wins.
    per_module: Vec<(String, LevelFilter)>,
}

impl CoreFileLogger {
    fn new(file: std::fs::File, directives: LogDirectives) -> Self {
        let mut per_module = default_quiet_modules();
        per_module.extend(directives.per_module);
        Self {
            file: std::sync::Mutex::new(file),
            global: directives.global,
            per_module,
        }
    }

    fn level_for(&self, target: &str) -> LevelFilter {
        self.per_module
            .iter()
            .rev()
            .find(|(module, _)| {
                target == module
                    || target
                        .strip_prefix(module.as_str())
                        .is_some_and(|rest| rest.starts_with("::"))
            })
            .map_or(self.global, |(_, level)| *level)
    }

    fn max_level(&self) -> LevelFilter {
        self.per_module
            .iter()
            .map(|(_, level)| *level)
            .fold(self.global, std::cmp::max)
    }
}

impl log::Log for CoreFileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= self.level_for(metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{}[{}][{}] {}\n",
            chrono::Utc::now().format("[%Y-%m-%d][%H:%M:%S]"),
            record.target(),
            record.level(),
            record.args()
        );
        if let Ok(mut file) = self.file.lock() {
            let _ = std::io::Write::write_all(&mut *file, line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = std::io::Write::flush(&mut *file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                .any(|(module, level)| module == "tauri_plugin_updater"
                    && *level == LevelFilter::Info),
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
        // `se_manager=debug` must NOT raise other crates: global stays at
        // the floor, the module gets its own override.
        let d = parse_directives("se_manager=debug");
        assert_eq!(d.global, default_floor());
        assert_eq!(
            d.per_module,
            vec![("se_manager".to_string(), LevelFilter::Debug)]
        );
    }

    #[test]
    fn multiple_modules_are_scoped_independently() {
        let d = parse_directives("hyper=warn,se_manager=trace");
        assert_eq!(d.global, default_floor());
        assert_eq!(
            d.per_module,
            vec![
                ("hyper".to_string(), LevelFilter::Warn),
                ("se_manager".to_string(), LevelFilter::Trace),
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
        let d = parse_directives("info,se_manager=trace");
        assert_eq!(d.global, LevelFilter::Info);
        assert_eq!(
            d.per_module,
            vec![("se_manager".to_string(), LevelFilter::Trace)]
        );
    }

    #[test]
    fn unrecognized_level_tokens_are_ignored() {
        let d = parse_directives("bogus,se_manager=nonsense");
        assert_eq!(d.global, default_floor());
        assert!(d.per_module.is_empty());
    }

    #[test]
    fn core_log_file_rotates_an_oversized_file_before_appending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("se-manager-acp-core.log");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![b'x'; MAX_LOG_FILE_SIZE as usize + 1]).unwrap();

        let file = open_core_log_file(&path).unwrap();

        assert_eq!(file.metadata().unwrap().len(), 0);
        let rotated = path.with_extension("old.log");
        assert_eq!(
            u128::from(std::fs::metadata(&rotated).unwrap().len()),
            MAX_LOG_FILE_SIZE + 1
        );
    }

    #[test]
    fn core_log_file_appends_below_the_rotation_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("se-manager-terminal-core.log");
        std::fs::write(&path, b"earlier run\n").unwrap();

        let mut file = open_core_log_file(&path).unwrap();
        std::io::Write::write_all(&mut file, b"this run\n").unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "earlier run\nthis run\n"
        );
        assert!(!path.with_extension("old.log").exists());
    }

    #[test]
    fn core_file_logger_formats_lines_and_scopes_levels_by_module() {
        use log::Log;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("core.log");
        let logger = CoreFileLogger::new(
            open_core_log_file(&path).unwrap(),
            LogDirectives {
                global: LevelFilter::Info,
                per_module: vec![("noisy".to_string(), LevelFilter::Warn)],
            },
        );
        let emit = |target: &str, level: log::Level, message: &str| {
            logger.log(
                &log::Record::builder()
                    .target(target)
                    .level(level)
                    .args(format_args!("{message}"))
                    .build(),
            );
        };

        emit("se_manager::core", log::Level::Info, "kept");
        emit("se_manager::core", log::Level::Debug, "below global");
        emit("noisy::inner", log::Level::Info, "below module");
        emit("noisy", log::Level::Warn, "module warn");
        emit("noisyneighbor", log::Level::Info, "not the noisy module");
        logger.flush();

        let lines: Vec<String> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            // Drop the fixed-width `[YYYY-MM-DD][HH:MM:SS]` stamp.
            .map(|line| line[22..].to_string())
            .collect();
        assert_eq!(
            lines,
            vec![
                "[se_manager::core][INFO] kept",
                "[noisy][WARN] module warn",
                "[noisyneighbor][INFO] not the noisy module",
            ]
        );
        let first = std::fs::read_to_string(&path).unwrap();
        let stamp = first.lines().next().unwrap();
        assert!(
            stamp.len() > 22 && stamp.as_bytes()[0] == b'[' && &stamp[11..13] == "][",
            "line starts with [date][time]: {stamp}"
        );
    }
}
