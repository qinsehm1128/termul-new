#[cfg(target_os = "windows")]
pub mod git_bash_paths {
    /// Primary Git Bash installation paths (Program Files)
    pub const PRIMARY_PATHS: &[&str] = &[
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\usr\bin\bash.exe",
    ];

    /// Fallback Git Bash paths for non-standard installations
    pub const FALLBACK_PATHS: &[&str] = &[
        r"C:\tools\msys64\usr\bin\bash.exe",
        r"C:\msys64\usr\bin\bash.exe",
        r"C:\Git\bin\bash.exe",
        r"C:\Git\usr\bin\bash.exe",
    ];
}

/// Unix shell lookup shared by detection and spawn resolution.
/// `$SHELL` is consulted first so Se matches Ghostty / Terminal.app.
#[cfg(not(target_os = "windows"))]
pub mod unix_shell_paths {
    use std::env;
    use std::path::Path;

    /// Homebrew first, then system prefixes. `$SHELL` is still preferred separately.
    pub const PREFIXES: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/bin", "/usr/bin"];

    /// Resolve a shell name (`zsh`, `bash`) to the same binary the login session uses.
    pub fn resolve_name(shell: &str) -> Option<String> {
        let trimmed = shell.trim();
        if trimmed.is_empty() || trimmed.contains('/') {
            return None;
        }

        if let Ok(login) = env::var("SHELL") {
            if Path::new(&login).file_name().and_then(|s| s.to_str()) == Some(trimmed)
                && Path::new(&login).exists()
            {
                return Some(login);
            }
        }

        for prefix in PREFIXES {
            let candidate = format!("{prefix}/{trimmed}");
            if Path::new(&candidate).exists() {
                return Some(candidate);
            }
        }

        None
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod unix_shell_path_tests {
    use super::unix_shell_paths::resolve_name;
    use std::path::Path;

    #[test]
    fn resolve_name_prefers_login_shell_when_basename_matches() {
        let Ok(login) = std::env::var("SHELL") else {
            return;
        };
        let Some(name) = Path::new(&login).file_name().and_then(|s| s.to_str()) else {
            return;
        };
        if !Path::new(&login).exists() {
            return;
        }
        assert_eq!(resolve_name(name).as_deref(), Some(login.as_str()));
    }
}
