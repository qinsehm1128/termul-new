//! Vendor transcript roots. Env overrides match Claude / Codex / Gemini /
//! OpenCode / pi conventions; clients cannot supply these paths.

use std::env;
use std::path::{Path, PathBuf};

use super::types::CliSessionAgentId;

pub fn user_home() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn claude_projects_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("projects"));
    }
    user_home().map(|home| home.join(".claude").join("projects"))
}

pub fn default_codex_home() -> Option<PathBuf> {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| user_home().map(|home| home.join(".codex")))
}

pub fn gemini_tmp_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("GEMINI_HOME") {
        return Some(PathBuf::from(dir).join("tmp"));
    }
    user_home().map(|home| home.join(".gemini").join("tmp"))
}

pub fn cursor_projects_dir() -> Option<PathBuf> {
    user_home().map(|home| home.join(".cursor").join("projects"))
}

pub fn opencode_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dir) = env::var_os("OPENCODE_HOME") {
        roots.push(PathBuf::from(dir));
    }
    if let Some(xdg) = env::var_os("XDG_DATA_HOME") {
        roots.push(PathBuf::from(xdg).join("opencode"));
    }
    if let Some(home) = user_home() {
        roots.push(home.join(".local").join("share").join("opencode"));
    }
    roots
}

pub fn pi_sessions_dir() -> Option<PathBuf> {
    env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| user_home().map(|home| home.join(".pi").join("agent").join("sessions")))
}

pub fn roots_for_agent(agent: CliSessionAgentId) -> Vec<PathBuf> {
    match agent {
        CliSessionAgentId::ClaudeCode => claude_projects_dir().into_iter().collect(),
        CliSessionAgentId::Codex => default_codex_home()
            .map(|home| home.join("sessions"))
            .into_iter()
            .collect(),
        CliSessionAgentId::GeminiCli => gemini_tmp_dir().into_iter().collect(),
        CliSessionAgentId::Cursor => cursor_projects_dir().into_iter().collect(),
        CliSessionAgentId::Opencode => opencode_roots()
            .into_iter()
            .flat_map(|root| [root.join("storage"), root])
            .collect(),
        CliSessionAgentId::Pi => pi_sessions_dir().into_iter().collect(),
    }
}

pub fn is_under_dir(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

pub fn normalize_abs_cwd(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy().replace('\\', "/");
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        PathBuf::from(path)
    } else {
        PathBuf::from(trimmed)
    }
}

pub fn encode_claude_project_dir(cwd: &Path) -> String {
    normalize_abs_cwd(cwd).to_string_lossy().replace('/', "-")
}

pub fn encode_cursor_project_dir(cwd: &Path) -> String {
    normalize_abs_cwd(cwd)
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "-")
}

pub fn encode_pi_project_dir(cwd: &Path) -> String {
    format!("--{}--", encode_cursor_project_dir(cwd))
}

fn cwd_candidates(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![normalize_abs_cwd(path)];
    if let Ok(canonical) = path.canonicalize() {
        let normalized = normalize_abs_cwd(&canonical);
        if !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    out
}

fn join_if_dir(base: &Path, name: &str) -> Option<PathBuf> {
    let path = base.join(name);
    path.is_dir().then_some(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkRoot {
    pub path: PathBuf,
    pub cwd: Option<String>,
}

fn push_unique(out: &mut Vec<WalkRoot>, path: PathBuf, cwd: Option<String>) {
    if out.iter().any(|existing| existing.path == path) {
        return;
    }
    out.push(WalkRoot { path, cwd });
}

/// Restrict the walk to vendor folders that encode the current project path.
/// Empty `scope_paths` means "all sessions" and returns the agent store roots.
pub fn walk_roots_for_agent(agent: CliSessionAgentId, scope_paths: &[PathBuf]) -> Vec<WalkRoot> {
    let bases = roots_for_agent(agent);
    if scope_paths.is_empty() {
        return bases
            .into_iter()
            .map(|path| WalkRoot { path, cwd: None })
            .collect();
    }
    let mut out = Vec::new();
    match agent {
        CliSessionAgentId::ClaudeCode => {
            for base in &bases {
                for scope in scope_paths {
                    for candidate in cwd_candidates(scope) {
                        if let Some(dir) = join_if_dir(base, &encode_claude_project_dir(&candidate))
                        {
                            push_unique(&mut out, dir, Some(candidate.display().to_string()));
                        }
                    }
                }
            }
        }
        CliSessionAgentId::Cursor => {
            for base in &bases {
                for scope in scope_paths {
                    for candidate in cwd_candidates(scope) {
                        if let Some(dir) = join_if_dir(base, &encode_cursor_project_dir(&candidate))
                        {
                            let transcripts = dir.join("agent-transcripts");
                            let chosen = if transcripts.is_dir() {
                                transcripts
                            } else {
                                dir
                            };
                            push_unique(&mut out, chosen, Some(candidate.display().to_string()));
                        }
                    }
                }
            }
        }
        CliSessionAgentId::Pi => {
            for base in &bases {
                for scope in scope_paths {
                    for candidate in cwd_candidates(scope) {
                        if let Some(dir) = join_if_dir(base, &encode_pi_project_dir(&candidate)) {
                            push_unique(&mut out, dir, Some(candidate.display().to_string()));
                        }
                    }
                }
            }
        }
        CliSessionAgentId::GeminiCli => {
            for base in &bases {
                for scope in scope_paths {
                    if let Some(name) = scope.file_name() {
                        if let Some(dir) = join_if_dir(base, &name.to_string_lossy()) {
                            push_unique(&mut out, dir, Some(scope.display().to_string()));
                        }
                    }
                }
            }
        }
        // Codex / OpenCode stores are not indexed by project path. Skip them
        // unless the caller asked for every session.
        CliSessionAgentId::Codex | CliSessionAgentId::Opencode => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The expected strings are golden values: they mirror how Claude Code,
    /// Cursor and pi actually name their per-project folders on disk, which is
    /// the whole reason these encoders exist. Keep the input a fixed synthetic
    /// path — deriving it from `current_dir()` would force the expectation to
    /// be derived too, and computing it with the function under test turns the
    /// assertion into `f(x) == f(x)`.
    #[test]
    fn encodes_vendor_project_folders() {
        let cwd = PathBuf::from("/Users/dev/projects/termul");
        assert_eq!(
            encode_claude_project_dir(&cwd),
            "-Users-dev-projects-termul"
        );
        assert_eq!(
            encode_cursor_project_dir(&cwd),
            "Users-dev-projects-termul"
        );
        assert_eq!(
            encode_pi_project_dir(&cwd),
            "--Users-dev-projects-termul--"
        );
    }

    #[test]
    fn scoped_roots_only_open_matching_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let keep = tmp.path().join("-Users-dev-projects-termul");
        let other = tmp.path().join("-Users-dev-other");
        fs::create_dir_all(&keep).unwrap();
        fs::create_dir_all(other).unwrap();
        let scope = [PathBuf::from("/Users/dev/projects/termul")];
        let mut roots = Vec::new();
        if let Some(dir) = join_if_dir(tmp.path(), &encode_claude_project_dir(&scope[0])) {
            roots.push(dir);
        }
        assert_eq!(roots, vec![keep]);
    }
}
