//! Refresh `Path` / `PATH` from OS sources before PTY spawn.
//!
//! Termul's GUI process keeps a snapshot of the environment from launch time.
//! Global installs and registry updates are invisible until we re-read PATH here.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Process-lifetime cache for the OS-probed PATH, so `fresh_path()` does not
/// spawn a login shell (Unix) or read the registry (Windows) on every agent
/// launch. Matches the `RG_PATH_CACHE` pattern in `commands.rs`.
static FRESH_PATH_CACHE: OnceLock<Option<String>> = OnceLock::new();

#[cfg(target_os = "windows")]
fn has_path_key(env: &HashMap<String, String>) -> bool {
    env.keys().any(|k| k.eq_ignore_ascii_case("path"))
}

#[cfg(target_os = "windows")]
fn get_path_from_map(env: &HashMap<String, String>) -> String {
    env.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("path"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn set_path_in_map(env: &mut HashMap<String, String>, value: String) {
    if let Some(existing_key) = env.keys().find(|k| k.eq_ignore_ascii_case("path")).cloned() {
        env.remove(&existing_key);
    }
    env.insert("Path".to_string(), value);
}

/// Merge `registry` and `inherited` PATH segments (platform delimiter), keeping
/// registry order first then appending inherited segments not already present.
pub fn merge_path_segments(registry: &str, inherited: &str, delimiter: char) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();

    // Semicolon-separated paths follow Windows rules (case-insensitive dedupe).
    let case_insensitive = delimiter == ';';

    let mut push_segment = |seg: &str| {
        let trimmed = seg.trim();
        if trimmed.is_empty() {
            return;
        }
        let key = if case_insensitive {
            trimmed.to_ascii_lowercase()
        } else {
            trimmed.to_string()
        };
        if seen.insert(key) {
            out.push(trimmed.to_string());
        }
    };

    for seg in registry.split(delimiter) {
        push_segment(seg);
    }
    for seg in inherited.split(delimiter) {
        push_segment(seg);
    }

    out.join(&delimiter.to_string())
}

#[cfg(target_os = "windows")]
fn expand_windows_env_value(value: &str) -> String {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::processenv::ExpandEnvironmentStringsW;

    if value.is_empty() {
        return String::new();
    }

    let wide: Vec<u16> = OsStr::new(value).encode_wide().chain(Some(0)).collect();
    let mut buf = vec![0u16; 32_768];
    unsafe {
        let needed = ExpandEnvironmentStringsW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32);
        if needed == 0 || needed as usize > buf.len() {
            return value.to_string();
        }
        let len = needed.saturating_sub(1) as usize;
        String::from_utf16_lossy(&buf[..len])
    }
}

#[cfg(target_os = "windows")]
fn read_windows_registry_path() -> Option<String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let machine = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
        .ok()
        .and_then(|k| k.get_value::<String, _>("Path").ok())
        .map(|s| expand_windows_env_value(&s))
        .filter(|s| !s.is_empty());

    let user = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Environment")
        .ok()
        .and_then(|k| k.get_value::<String, _>("Path").ok())
        .map(|s| expand_windows_env_value(&s))
        .filter(|s| !s.is_empty());

    match (machine, user) {
        (Some(m), Some(u)) => Some(merge_path_segments(&m, &u, ';')),
        (Some(m), None) => Some(m),
        (None, Some(u)) => Some(u),
        (None, None) => None,
    }
}

/// PATH string for executable resolution (registry/login probe, else process env).
pub fn path_for_resolution() -> std::ffi::OsString {
    fresh_path()
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default()
}

/// Returns the refreshed PATH string for the current platform, if obtainable.
/// Cached for the process lifetime via `FRESH_PATH_CACHE` so the probe runs at
/// most once; subsequent calls return the cached result without spawning a
/// login shell or reading the registry.
pub fn fresh_path() -> Option<String> {
    FRESH_PATH_CACHE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                read_windows_registry_path()
            }

            #[cfg(not(target_os = "windows"))]
            {
                probe_unix_login_path()
            }
        })
        .clone()
}

/// Sentinels bracketing the probe's payload.
///
/// An interactive shell runs the user's rc file, and rc files print: version
/// managers announce themselves, Powerlevel10k emits an instant prompt, `motd`
/// helpers cat a banner. All of that lands on stdout ahead of the PATH. Reading
/// the whole of stdout — which is what this probe used to do — turns any such
/// line into a PATH segment.
pub const PATH_PROBE_BEGIN: &str = "__TERMUL_PATH_BEGIN__";
pub const PATH_PROBE_END: &str = "__TERMUL_PATH_END__";

/// How long the probe waits for the user's shell before giving up. An rc file
/// can block on anything — a slow version manager, a network-backed prompt, a
/// `read` nobody will answer — and this runs on the way to spawning an agent.
#[cfg(not(target_os = "windows"))]
const PATH_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Pull the payload out of whatever the shell printed around it.
pub fn extract_marked_path(raw: &str) -> Option<String> {
    // Last begin, then the first end after it. An rc file that echoes the command
    // it is about to run (`set -x`, or a trace-happy login banner) prints both
    // markers before any real output, so the real payload is always the final
    // pair. Anchoring on the first begin and the last end instead spans across
    // the echo and swallows it whole — which is what this used to do.
    let after_begin = raw.rfind(PATH_PROBE_BEGIN)? + PATH_PROBE_BEGIN.len();
    let rest = &raw[after_begin..];
    let end = rest.find(PATH_PROBE_END)?;
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Arguments and probe command for a shell, or `None` when its dialect is not
/// one this code can write correctly.
///
/// `-i` is the point of the exercise. zsh reads `.zshrc` only when interactive,
/// bash reads `.bashrc` only when interactive, and version managers (nvm, fnm,
/// rbenv, pyenv, asdf) install themselves there — a login-but-not-interactive
/// shell reports a PATH with the whole toolchain missing, which is exactly how
/// a GUI-launched Termul ended up unable to find `npx` while its own terminals
/// could. Matches `shell_startup_args`, which is why PTY terminals never had
/// this problem.
pub fn shell_path_probe(shell_name: &str) -> Option<(&'static [&'static str], String)> {
    // POSIX-family: `$PATH` is already a colon-joined string.
    let posix = || {
        format!("printf %s '{PATH_PROBE_BEGIN}'; printf %s \"$PATH\"; printf %s '{PATH_PROBE_END}'")
    };

    match shell_name {
        // `-i -l -c` rather than `-ilc`: csh-family shells reject bundled flags,
        // and keeping the two families visibly different stops a later edit from
        // "simplifying" them back together.
        "bash" | "zsh" | "ksh" | "mksh" => Some((&["-i", "-l", "-c"], posix())),
        // fish's `$PATH` is a list, so it has to be joined explicitly. `string
        // join` adds a trailing newline; `extract_marked_path` trims it.
        "fish" => Some((
            &["-i", "-l", "-c"],
            format!(
                "printf %s '{PATH_PROBE_BEGIN}'; string join : $PATH; printf %s '{PATH_PROBE_END}'"
            ),
        )),
        // csh and tcsh have no `-l` that combines with `-c`.
        "csh" | "tcsh" => Some((&["-ic"], posix())),
        // Deliberately unlisted: nu and xonsh need their own dialect for both the
        // join and the quoting, and neither is verifiable from here. They keep
        // the previous behaviour — no probe, process PATH only — rather than
        // getting a guessed command that could return a corrupted PATH.
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn probe_unix_login_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    probe_shell_path(&shell, &[])
}

/// The probe itself, against an explicit shell and an explicit environment
/// overlay.
///
/// Both are parameters rather than reads of the ambient process environment so
/// a test can point a real shell at a throwaway HOME. `std::env::set_var` would
/// not do: cargo runs tests as threads in one process, so mutating HOME for the
/// duration of this probe breaks whatever unrelated test happens to be resolving
/// a path at the same moment — which is exactly what it did.
#[cfg(not(target_os = "windows"))]
pub(crate) fn probe_shell_path(shell: &str, env_overrides: &[(&str, &str)]) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let shell_name = Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("sh");

    let (args, command) = shell_path_probe(shell_name)?;

    let mut builder = Command::new(shell);
    builder
        .args(args)
        .arg(&command)
        // An interactive shell that reaches a `read` or a pager with an inherited
        // stdin waits forever; with /dev/null it gets EOF and moves on.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Piped, not inherited: rc-file noise on stderr would otherwise be
        // written to whatever fd the GUI process happens to have.
        .stderr(Stdio::piped())
        // Lets an rc file skip work it only needs for a real session. Mirrors
        // VS Code's `VSCODE_RESOLVING_ENVIRONMENT`.
        .env("TERMUL_RESOLVING_ENVIRONMENT", "1");
    for (key, value) in env_overrides {
        builder.env(key, value);
    }
    let mut child = builder.spawn().ok()?;

    let status = wait_with_timeout(&mut child, PATH_PROBE_TIMEOUT);

    let mut raw = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut raw);
    }

    match status {
        Some(status) if status.success() => extract_marked_path(&raw),
        Some(_) => None,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            log::warn!(
                "[env-refresh] {shell_name} PATH probe timed out after {:?}; \
                 falling back to the process PATH",
                PATH_PROBE_TIMEOUT
            );
            None
        }
    }
}

/// `None` on timeout. The caller owns killing the child, so the poll loop stays
/// free of the cleanup ordering.
#[cfg(not(target_os = "windows"))]
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> Option<std::process::ExitStatus> {
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if started.elapsed() >= timeout => return None,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(_) => return None,
        }
    }
}

/// Apply a refreshed PATH to `env`, preserving custom overrides already present.
pub fn apply_fresh_path(env: &mut HashMap<String, String>) {
    let delimiter = if cfg!(target_os = "windows") {
        ';'
    } else {
        ':'
    };

    let inherited = {
        #[cfg(target_os = "windows")]
        {
            if has_path_key(env) {
                get_path_from_map(env)
            } else {
                std::env::var("PATH").unwrap_or_default()
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            env.get("PATH")
                .cloned()
                .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
        }
    };

    let Some(registry_or_probed) = fresh_path() else {
        return;
    };

    let merged = merge_path_segments(&registry_or_probed, &inherited, delimiter);

    #[cfg(target_os = "windows")]
    set_path_in_map(env, merged);

    #[cfg(not(target_os = "windows"))]
    env.insert("PATH".to_string(), merged);
}

const LOCALE_KEYS: [&str; 3] = ["LANG", "LC_ALL", "LC_CTYPE"];

fn locale_value_is_utf8(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    upper.contains("UTF-8") || upper.contains("UTF8")
}

/// Fallback language tag when the process inherited no locale (Dock / `open`).
pub fn default_utf8_lang() -> &'static str {
    "en_US.UTF-8"
}

/// Force the codeset of a locale string to UTF-8. `C` / `POSIX` become
/// [`default_utf8_lang`]; `zh_CN` / `zh_CN.GBK` become `zh_CN.UTF-8`.
pub fn normalize_utf8_locale(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return default_utf8_lang().to_string();
    }
    if locale_value_is_utf8(trimmed) {
        return trimmed.to_string();
    }
    if trimmed.eq_ignore_ascii_case("C") || trimmed.eq_ignore_ascii_case("POSIX") {
        return default_utf8_lang().to_string();
    }
    let language = trimmed.split(['.', '@']).next().unwrap_or(trimmed);
    if language.is_empty() {
        return default_utf8_lang().to_string();
    }
    format!("{language}.UTF-8")
}

fn fill_if_empty(env: &mut HashMap<String, String>, key: &str, value: &str) {
    if env
        .get(key)
        .map(|existing| existing.trim().is_empty())
        .unwrap_or(true)
    {
        env.insert(key.to_string(), value.to_string());
    }
}

/// GUI / agent parents often export `NO_COLOR` or `FORCE_COLOR=0`. A real PTY
/// is color-capable; those flags must not be inherited into the child.
pub fn is_inherited_color_suppressor(key: &str, value: &str) -> bool {
    if key.eq_ignore_ascii_case("NO_COLOR") || key.eq_ignore_ascii_case("NODE_DISABLE_COLORS") {
        return true;
    }
    if key.eq_ignore_ascii_case("FORCE_COLOR") {
        let trimmed = value.trim();
        return trimmed.is_empty() || trimmed == "0" || trimmed.eq_ignore_ascii_case("false");
    }
    false
}

/// Process environment minus inherited color-suppression flags.
pub fn inherited_process_env() -> impl Iterator<Item = (String, String)> {
    std::env::vars().filter(|(key, value)| !is_inherited_color_suppressor(key, value))
}

/// Advertise 24-bit color to chalk, ls, git, and Node `hasColors()`.
/// Does not overwrite keys the project already set (including `FORCE_COLOR=0`).
pub fn apply_color_capability(env: &mut HashMap<String, String>) {
    fill_if_empty(env, "TERM", "xterm-256color");
    fill_if_empty(env, "COLORTERM", "truecolor");
    fill_if_empty(env, "FORCE_COLOR", "3");
    fill_if_empty(env, "CLICOLOR", "1");
    fill_if_empty(env, "CLICOLOR_FORCE", "1");
}

/// GUI-launched apps often inherit no `LANG` / `LC_*` (or inherit `C`).
/// libc then uses the POSIX C locale and CJK / Python output becomes mojibake.
///
/// Upgrades existing non-UTF-8 locale keys in place, fills `LANG` / `LC_CTYPE`
/// when missing, and sets Python UTF-8 stdio defaults when unset. Project
/// `custom_env` applied after this can still override any of these keys.
pub fn apply_utf8_locale(env: &mut HashMap<String, String>) {
    for key in LOCALE_KEYS {
        if let Some(value) = env.get(key).cloned() {
            if value.trim().is_empty() || locale_value_is_utf8(&value) {
                continue;
            }
            env.insert(key.to_string(), normalize_utf8_locale(&value));
        }
    }

    fill_if_empty(env, "LANG", default_utf8_lang());
    // LC_CTYPE outranks LANG under POSIX (LC_ALL > LC_CTYPE > LANG), so filling
    // a bare "UTF-8" here downgraded users who already export a correct LANG,
    // and on glibc a bare "UTF-8" is not a valid locale name at all (setlocale
    // falls back to C/POSIX and wcwidth stops returning 2 for CJK). Mirror the
    // effective LANG instead. Read it only after the fill above, so the value
    // seen here is already normalised to a full *.UTF-8 name.
    // Evidence: U7, see the session field-evidence file.
    let ctype = env
        .get("LANG")
        .map(String::to_owned)
        .unwrap_or_else(|| default_utf8_lang().to_string());
    fill_if_empty(env, "LC_CTYPE", &ctype);
    fill_if_empty(env, "PYTHONUTF8", "1");
    fill_if_empty(env, "PYTHONIOENCODING", "utf-8");
}

/// Startup flags so profile + rc files load the same way as Ghostty / Terminal.app.
/// bash/zsh need both login (`-l`) and interactive (`-i`); otherwise `.zshrc` is skipped.
// Only invoked from the non-Windows PTY spawn path; on Windows it is exercised
// solely by unit tests, so a non-test Windows build sees it as unused.
#[cfg_attr(windows, allow(dead_code))]
pub fn shell_startup_args(shell_path: &str) -> &'static [&'static str] {
    let name = Path::new(shell_path)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match name.as_str() {
        "bash" | "zsh" => &["-l", "-i"],
        "fish" => &["-l"],
        #[cfg(not(target_os = "windows"))]
        "pwsh" | "powershell" => &["-Login"],
        _ => &[],
    }
}

/// Whether an interactive shell spawn should pass a login-shell flag.
#[cfg(test)]
pub fn shell_wants_login_arg(shell_path: &str) -> Option<&'static str> {
    shell_startup_args(shell_path).first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_asks_bash_and_zsh_for_an_interactive_shell() {
        // The whole bug in one assertion. zsh sources `.zshrc` only when
        // interactive, and that is where nvm/fnm/rbenv put themselves, so a
        // probe without `-i` reports a PATH with no `npx` in it while the app's
        // own terminals — which do pass `-i` — can see it.
        for shell in ["bash", "zsh"] {
            let (args, _) = shell_path_probe(shell).expect("{shell} must be probed");
            assert!(args.contains(&"-i"), "{shell} probe must be interactive");
            assert!(args.contains(&"-l"), "{shell} probe must be a login shell");
        }
    }

    #[test]
    fn probe_matches_the_flags_the_pty_already_uses() {
        // Terminals never had this bug because `shell_startup_args` passes both
        // flags. Pinning the two together is what stops them drifting apart
        // again — which is exactly how the app ended up disagreeing with itself
        // about what the user's PATH was.
        let pty = shell_startup_args("/bin/zsh");
        let (probe, _) = shell_path_probe("zsh").expect("zsh must be probed");
        for flag in pty {
            assert!(
                probe.contains(flag),
                "PTY passes {flag} but the PATH probe does not"
            );
        }
    }

    #[test]
    fn probe_joins_the_fish_path_list() {
        let (args, command) = shell_path_probe("fish").expect("fish must be probed");
        assert!(args.contains(&"-i"));
        // `$PATH` is a list in fish, so the POSIX `printf %s "$PATH"` would emit
        // it space-separated and every segment after the first would be lost.
        assert!(
            command.contains("string join : $PATH"),
            "fish probe must join the list explicitly, got: {command}"
        );
    }

    #[test]
    fn probe_does_not_pass_login_to_csh_family() {
        for shell in ["csh", "tcsh"] {
            let (args, _) = shell_path_probe(shell).expect("{shell} must be probed");
            assert_eq!(args, ["-ic"], "{shell} rejects -l combined with -c");
        }
    }

    #[test]
    fn probe_declines_shells_whose_dialect_is_unverified() {
        // Returning None keeps the previous behaviour (process PATH only). A
        // guessed command would be worse than no probe: it can succeed and
        // return a corrupted PATH.
        for shell in ["nu", "xonsh", "elvish"] {
            assert!(shell_path_probe(shell).is_none(), "{shell} must not guess");
        }
    }

    #[test]
    fn marked_path_survives_a_chatty_rc_file() {
        let raw = format!(
            "nvm: v25.9.0\n\u{1b}]0;title\u{7}p10k instant prompt\n{PATH_PROBE_BEGIN}/opt/homebrew/bin:/usr/bin{PATH_PROBE_END}"
        );
        assert_eq!(
            extract_marked_path(&raw).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
    }

    #[test]
    fn marked_path_trims_the_newline_fish_appends() {
        let raw = format!("{PATH_PROBE_BEGIN}/usr/local/bin:/usr/bin\n{PATH_PROBE_END}");
        assert_eq!(
            extract_marked_path(&raw).as_deref(),
            Some("/usr/local/bin:/usr/bin")
        );
    }

    #[test]
    fn marked_path_takes_the_last_close_when_the_command_is_echoed() {
        // `set -x` in an rc file replays the probe command itself, so both
        // markers appear before any real output.
        let raw = format!(
            "+ printf %s {PATH_PROBE_BEGIN} ... {PATH_PROBE_END}\n{PATH_PROBE_BEGIN}/real/bin{PATH_PROBE_END}"
        );
        assert_eq!(extract_marked_path(&raw).as_deref(), Some("/real/bin"));
    }

    #[test]
    fn marked_path_rejects_output_without_markers() {
        // The pre-fix probe took all of stdout, so a banner became a PATH
        // segment. No markers must mean no PATH, not a corrupted one.
        assert!(extract_marked_path("command not found: nvm").is_none());
        assert!(extract_marked_path("").is_none());
        assert!(extract_marked_path(&format!("{PATH_PROBE_BEGIN}   {PATH_PROBE_END}")).is_none());
    }

    /// Drives a real zsh against a throwaway HOME laid out the way the bug
    /// requires: the toolchain on `.zshrc` (where nvm/fnm/rbenv install
    /// themselves) and something else entirely on `.zprofile`. A probe that is
    /// login-but-not-interactive sees only the `.zprofile` half.
    ///
    /// Not a mock. The previous probe passed every unit test that existed while
    /// being wrong on every real machine, because nothing ever ran a shell.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn real_zsh_probe_sees_the_interactive_rc_file() {
        use std::io::Write;

        let zsh = "/bin/zsh";
        if !Path::new(zsh).exists() {
            eprintln!("skipping: {zsh} not present");
            return;
        }

        let home = std::env::temp_dir().join(format!("termul-env-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("temp HOME");

        // `.zprofile` runs for any login shell; `.zshrc` only when interactive.
        let mut zprofile = std::fs::File::create(home.join(".zprofile")).expect(".zprofile");
        writeln!(zprofile, "export PATH=/termul-profile-only:$PATH").unwrap();
        let mut zshrc = std::fs::File::create(home.join(".zshrc")).expect(".zshrc");
        // The banner is the second half of the test: it proves the sentinels do
        // their job, because the pre-fix probe read all of stdout.
        writeln!(zshrc, "echo 'nvm loaded, welcome back'").unwrap();
        writeln!(zshrc, "export PATH=/termul-rc-only:$PATH").unwrap();

        // Passed to the child only. Mutating the process HOME here would reach
        // every test cargo is running on another thread at the same time.
        let home_str = home.to_string_lossy().into_owned();
        let probed = probe_shell_path(
            zsh,
            // ZDOTDIR as well as HOME: zsh consults it first, so a developer
            // machine that sets it would send the probe to the real dotfiles.
            &[("HOME", home_str.as_str()), ("ZDOTDIR", home_str.as_str())],
        );

        let _ = std::fs::remove_dir_all(&home);

        let probed = probed.expect("probe must return a PATH");
        assert!(
            probed.contains("/termul-rc-only"),
            "probe missed the .zshrc PATH — this is the nvm case: {probed}"
        );
        assert!(
            probed.contains("/termul-profile-only"),
            "probe must keep the .zprofile PATH too: {probed}"
        );
        assert!(
            !probed.contains("welcome back"),
            "rc-file banner leaked into the PATH: {probed}"
        );
    }

    #[test]
    fn merge_dedupes_case_insensitively_on_windows_style() {
        let merged = merge_path_segments(r"C:\Tools;C:\App", r"C:\tools;C:\Extra", ';');
        assert_eq!(merged, r"C:\Tools;C:\App;C:\Extra");
    }

    #[test]
    fn merge_unix_colon_delimiter() {
        let merged = merge_path_segments("/usr/bin", "/bin:/usr/bin", ':');
        assert_eq!(merged, "/usr/bin:/bin");
    }

    #[test]
    fn merge_skips_empty_segments() {
        let merged = merge_path_segments(";;/a", "/b;;", ';');
        assert_eq!(merged, "/a;/b");
    }

    #[test]
    fn shell_login_arg_for_bash() {
        assert_eq!(shell_wants_login_arg("/usr/bin/bash"), Some("-l"));
    }

    #[test]
    fn shell_startup_args_for_zsh_are_login_and_interactive() {
        assert_eq!(shell_startup_args("/opt/homebrew/bin/zsh"), ["-l", "-i"]);
    }

    #[test]
    fn shell_login_arg_for_cmd_none() {
        assert_eq!(shell_wants_login_arg("cmd.exe"), None);
    }

    #[test]
    fn normalize_upgrades_c_and_language_only_tags() {
        assert_eq!(normalize_utf8_locale("C"), default_utf8_lang());
        assert_eq!(normalize_utf8_locale("POSIX"), default_utf8_lang());
        assert_eq!(normalize_utf8_locale("zh_CN"), "zh_CN.UTF-8");
        assert_eq!(normalize_utf8_locale("zh_CN.GBK"), "zh_CN.UTF-8");
        assert_eq!(normalize_utf8_locale("en_US.UTF-8"), "en_US.UTF-8");
        assert_eq!(normalize_utf8_locale("  "), default_utf8_lang());
    }

    #[test]
    fn apply_utf8_locale_fills_missing_gui_env() {
        let mut env = HashMap::new();
        apply_utf8_locale(&mut env);
        assert_eq!(
            env.get("LANG").map(String::as_str),
            Some(default_utf8_lang())
        );
        assert_eq!(
            env.get("LC_CTYPE").map(String::as_str),
            Some(default_utf8_lang())
        );
        assert_eq!(env.get("PYTHONUTF8").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("PYTHONIOENCODING").map(String::as_str),
            Some("utf-8")
        );
    }

    #[test]
    fn apply_utf8_locale_upgrades_c_and_preserves_explicit_python() {
        let mut env = HashMap::from([
            ("LANG".to_string(), "C".to_string()),
            ("LC_ALL".to_string(), "C".to_string()),
            ("PYTHONUTF8".to_string(), "0".to_string()),
        ]);
        apply_utf8_locale(&mut env);
        assert_eq!(
            env.get("LANG").map(String::as_str),
            Some(default_utf8_lang())
        );
        assert_eq!(
            env.get("LC_ALL").map(String::as_str),
            Some(default_utf8_lang())
        );
        assert_eq!(env.get("PYTHONUTF8").map(String::as_str), Some("0"));
    }

    #[test]
    fn apply_utf8_locale_leaves_existing_utf8_lang() {
        let mut env = HashMap::from([("LANG".to_string(), "zh_CN.UTF-8".to_string())]);
        apply_utf8_locale(&mut env);
        assert_eq!(env.get("LANG").map(String::as_str), Some("zh_CN.UTF-8"));
        // R-07 / U7: this assertion previously pinned the downgrade - a user with
        // LANG=zh_CN.UTF-8 and no LC_CTYPE was given LC_CTYPE=UTF-8, which
        // outranks LANG. Tightening it to zh_CN.UTF-8 is the fix, not a relaxation.
        assert_eq!(env.get("LC_CTYPE").map(String::as_str), Some("zh_CN.UTF-8"));
    }

    #[test]
    fn apply_utf8_locale_ctype_mirrors_normalized_lang() {
        let mut env = HashMap::from([("LANG".to_string(), "zh_CN.GBK".to_string())]);
        apply_utf8_locale(&mut env);
        assert_eq!(env.get("LANG").map(String::as_str), Some("zh_CN.UTF-8"));
        assert_eq!(env.get("LC_CTYPE").map(String::as_str), Some("zh_CN.UTF-8"));
    }

    #[test]
    fn apply_utf8_locale_preserves_explicit_lc_ctype() {
        let mut env = HashMap::from([
            ("LANG".to_string(), "en_US.UTF-8".to_string()),
            ("LC_CTYPE".to_string(), "ja_JP.UTF-8".to_string()),
        ]);
        apply_utf8_locale(&mut env);
        assert_eq!(env.get("LANG").map(String::as_str), Some("en_US.UTF-8"));
        assert_eq!(env.get("LC_CTYPE").map(String::as_str), Some("ja_JP.UTF-8"));
    }

    #[test]
    fn apply_color_capability_fills_missing_and_keeps_explicit_off() {
        let mut env = HashMap::new();
        apply_color_capability(&mut env);
        assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));
        assert_eq!(env.get("FORCE_COLOR").map(String::as_str), Some("3"));
        assert_eq!(env.get("CLICOLOR_FORCE").map(String::as_str), Some("1"));

        let mut off = HashMap::from([("FORCE_COLOR".to_string(), "0".to_string())]);
        apply_color_capability(&mut off);
        assert_eq!(off.get("FORCE_COLOR").map(String::as_str), Some("0"));
    }

    #[test]
    fn inherited_color_suppressors() {
        assert!(is_inherited_color_suppressor("NO_COLOR", "1"));
        assert!(is_inherited_color_suppressor("no_color", ""));
        assert!(is_inherited_color_suppressor("NODE_DISABLE_COLORS", "1"));
        assert!(is_inherited_color_suppressor("FORCE_COLOR", "0"));
        assert!(is_inherited_color_suppressor("FORCE_COLOR", "false"));
        assert!(!is_inherited_color_suppressor("FORCE_COLOR", "3"));
        assert!(!is_inherited_color_suppressor("COLORTERM", "truecolor"));
    }
}
