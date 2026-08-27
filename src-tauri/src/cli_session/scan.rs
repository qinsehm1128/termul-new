use std::path::PathBuf;

use chrono::Utc;

use super::parse::{hydrate_session, is_allowed_transcript_path, scanned_session};
use super::paths::{default_codex_home, is_under_dir, walk_roots_for_agent};
use super::scope::filter_scope_paths;
use super::types::{
    CliSessionAgentId, CliSessionListArgs, CliSessionListResult, CliSessionResolveArgs,
    CliSessionResolveResult, CliSessionScanIssue, DiscoveredCliSession, DEFAULT_LIMIT_PER_AGENT,
    RESOLVE_BATCH_MAX, WALK_LIMIT_PER_AGENT,
};
use super::walk::{is_cursor_transcript, is_opencode_session_json, walk_session_files};

pub fn list_cli_sessions(
    args: CliSessionListArgs,
    allowed_scope_roots: Option<&[PathBuf]>,
) -> CliSessionListResult {
    let _force = args.force;
    let scanned_at = Utc::now().to_rfc3339();
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT_PER_AGENT).max(1);
    let agents = args
        .agents
        .clone()
        .unwrap_or_else(|| CliSessionAgentId::all().to_vec());
    let raw_scope = args.scope_paths.clone().unwrap_or_default();
    let (scope_paths, mut issues) = filter_scope_paths(&raw_scope, allowed_scope_roots);

    log::info!(
        target: "termul::cli_session",
        "operation=list_cli_sessions agents={} scope_paths={} limit={}",
        agents.len(),
        scope_paths.len(),
        limit
    );

    let mut sessions = Vec::new();
    for agent in agents {
        let (found, agent_issues) = scan_agent(agent, limit, &scope_paths);
        sessions.extend(found);
        issues.extend(agent_issues);
    }

    sessions.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.agent_id.as_str().cmp(right.agent_id.as_str()))
    });

    log::info!(
        target: "termul::cli_session",
        "operation=list_cli_sessions_done sessions={} issues={}",
        sessions.len(),
        issues.len()
    );

    CliSessionListResult {
        sessions,
        issues,
        scanned_at,
    }
}

fn scan_agent(
    agent: CliSessionAgentId,
    limit: usize,
    scope_paths: &[PathBuf],
) -> (Vec<DiscoveredCliSession>, Vec<CliSessionScanIssue>) {
    let mut issues = Vec::new();
    let roots = walk_roots_for_agent(agent, scope_paths);
    if roots.is_empty() {
        log::info!(
            target: "termul::cli_session",
            "operation=scan_root_missing agent={}",
            agent.as_str()
        );
        return (Vec::new(), issues);
    }

    let mut files = Vec::new();
    for root in &roots {
        log::info!(
            target: "termul::cli_session",
            "operation=scan_root agent={} root={}",
            agent.as_str(),
            root.path.display()
        );
        let walked = match agent {
            CliSessionAgentId::ClaudeCode => walk_session_files(
                &root.path,
                &["jsonl"],
                &["subagents"],
                None,
                WALK_LIMIT_PER_AGENT,
            ),
            CliSessionAgentId::Codex => {
                walk_session_files(&root.path, &["jsonl"], &[], None, WALK_LIMIT_PER_AGENT)
            }
            CliSessionAgentId::GeminiCli => walk_session_files(
                &root.path,
                &["json", "jsonl"],
                &[],
                None,
                WALK_LIMIT_PER_AGENT,
            ),
            CliSessionAgentId::Cursor => walk_session_files(
                &root.path,
                &["jsonl"],
                &[],
                Some(&is_cursor_transcript),
                WALK_LIMIT_PER_AGENT,
            ),
            CliSessionAgentId::Opencode => {
                if let Ok(entries) = std::fs::read_dir(&root.path) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".db") {
                            issues.push(CliSessionScanIssue {
                                agent_id: agent.as_str().to_string(),
                                path: entry.path().display().to_string(),
                                message: "OpenCode SQLite store found; JSON sessions are listed"
                                    .to_string(),
                            });
                        }
                    }
                }
                walk_session_files(
                    &root.path,
                    &["json"],
                    &[],
                    Some(&is_opencode_session_json),
                    WALK_LIMIT_PER_AGENT,
                )
            }
            CliSessionAgentId::Pi => {
                walk_session_files(&root.path, &["jsonl"], &[], None, WALK_LIMIT_PER_AGENT)
            }
        };
        for file in walked {
            files.push((file, root.cwd.clone()));
        }
    }

    files.sort_by(|left, right| right.0.modified.cmp(&left.0.modified));
    files.dedup_by(|left, right| left.0.path == right.0.path);

    let codex_home = default_codex_home();
    let mut matched = Vec::new();
    for (file, cwd) in files {
        if matched.len() >= limit {
            break;
        }
        let home = if agent == CliSessionAgentId::Codex {
            codex_home.as_ref().and_then(|home| {
                if is_default_codex_home(home) {
                    None
                } else {
                    Some(home.display().to_string())
                }
            })
        } else {
            None
        };
        matched.push(scanned_session(agent, &file, cwd, home));
    }

    (matched, issues)
}

pub fn resolve_cli_sessions(args: CliSessionResolveArgs) -> CliSessionResolveResult {
    let mut sessions = Vec::new();
    let mut issues = Vec::new();
    let codex_home = default_codex_home();
    for file in args.files.into_iter().take(RESOLVE_BATCH_MAX) {
        let path = PathBuf::from(&file.file_path);
        let home = if file.agent_id == CliSessionAgentId::Codex {
            codex_home.as_ref().and_then(|home| {
                if is_default_codex_home(home) {
                    None
                } else {
                    Some(home.display().to_string())
                }
            })
        } else {
            None
        };
        if !is_allowed_transcript_path(file.agent_id, &path) {
            issues.push(CliSessionScanIssue {
                agent_id: file.agent_id.as_str().to_string(),
                path: file.file_path,
                message: "transcript path is outside the host vendor store".to_string(),
            });
            continue;
        }
        match hydrate_session(file.agent_id, &path, None, home) {
            Some(session) => sessions.push(session),
            None => issues.push(CliSessionScanIssue {
                agent_id: file.agent_id.as_str().to_string(),
                path: file.file_path,
                message: "failed to read first session_id from transcript".to_string(),
            }),
        }
    }
    CliSessionResolveResult { sessions, issues }
}

fn is_default_codex_home(home: &std::path::Path) -> bool {
    std::env::var_os("CODEX_HOME").is_none()
        && super::paths::user_home().is_some_and(|user| {
            is_under_dir(home, &user.join(".codex")) || home == user.join(".codex")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn filename_only_lists_jsonl_names() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("sess-keep.jsonl"),
            "this body must not be read",
        )
        .unwrap();
        fs::write(dir.path().join("notes.txt"), "ignore").unwrap();
        let files = walk_session_files(dir.path(), &["jsonl"], &["subagents"], None, 100);
        assert_eq!(files.len(), 1);
        let session = scanned_session(
            CliSessionAgentId::ClaudeCode,
            &files[0],
            Some("/repo".into()),
            None,
        );
        assert_eq!(session.session_id, "");
        assert!(!session.resumable);
        assert_eq!(session.cwd.as_deref(), Some("/repo"));
    }
}
