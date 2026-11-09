//! Host-only transcript resolution. A client supplies terminal identity, never a file path.
use crate::{
    agent_hooks::{AgentHookEvent, AgentKind},
    cli_session::paths,
};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

pub fn resolve(event: &AgentHookEvent) -> Option<PathBuf> {
    let qin_home = std::env::var_os("QIN_CODE_HOME")
        .map(PathBuf::from)
        .or_else(|| paths::user_home().map(|p| p.join(".qin-code")));
    resolve_with(
        event,
        paths::claude_projects_dir(),
        paths::pi_sessions_dir(),
        qin_home,
    )
}

fn resolve_with(
    event: &AgentHookEvent,
    claude: Option<PathBuf>,
    pi: Option<PathBuf>,
    qin: Option<PathBuf>,
) -> Option<PathBuf> {
    // Hook normalization already selects transcript_path, then native session_file.
    if let Some(path) = &event.transcript_path {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            if event.agent == AgentKind::QinCode
                && path.extension().is_some_and(|ext| ext == "json")
            {
                return Some(path.with_extension("journal.jsonl"));
            }
            return Some(path);
        }
    }
    let id = &event.session_id;
    if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
        return None;
    }
    match event.agent {
        AgentKind::ClaudeCode => find(claude?, |path| {
            path.file_name().and_then(|s| s.to_str()) == Some(&format!("{id}.jsonl"))
        }),
        AgentKind::Pi => find(pi?, |path| {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            crate::memory_index::adapters::pi::is_transcript_name(name)
                && (name == format!("{id}.jsonl") || name.ends_with(&format!("_{id}.jsonl")))
        }),
        AgentKind::QinCode => {
            let root = qin?.join("sessions");
            let flat = root.join(format!("{id}.journal.jsonl"));
            if flat.is_file() {
                return Some(flat);
            }
            let direct = root.join(id).join("journal.jsonl");
            if direct.is_file() {
                return Some(direct);
            }
            find(root, |path| {
                path.file_name().is_some_and(|s| s == "journal.jsonl")
                    && path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|s| s.to_str())
                        == Some(id.as_str())
            })
        }
    }
}

fn find(root: PathBuf, matches: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut queue = VecDeque::from([(root, 0u8)]);
    let mut visited = 0;
    let mut found = None;
    while let Some((dir, depth)) = queue.pop_front() {
        if depth > 8 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > 50_000 {
                return None;
            } // ambiguity/bound exceeded, never guess
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                queue.push_back((path, depth + 1));
            } else if kind.is_file() && matches(&path) {
                if found.is_some() {
                    return None;
                }
                found = Some(path);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_hooks::HookKind;
    fn event(agent: AgentKind, id: &str) -> AgentHookEvent {
        AgentHookEvent {
            sequence: 1,
            terminal_id: "fixture".into(),
            project_id: "fixture".into(),
            agent,
            session_id: id.into(),
            transcript_path: None,
            kind: HookKind::Idle,
            at: 1,
            tool_name: None,
            ok: None,
        }
    }
    fn file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    #[test]
    fn hook_path_wins_and_qin_snapshot_maps_to_native_journal() {
        let root = tempfile::tempdir().unwrap();
        let mut e = event(AgentKind::Pi, "id");
        let path = root.path().join("explicit.jsonl");
        e.transcript_path = Some(path.to_string_lossy().into());
        assert_eq!(resolve_with(&e, None, None, None), Some(path));
        e.agent = AgentKind::QinCode;
        e.transcript_path = Some(root.path().join("session.json").to_string_lossy().into());
        assert_eq!(
            resolve_with(&e, None, None, None),
            Some(root.path().join("session.journal.jsonl"))
        );
    }
    #[test]
    fn resolves_native_claude_and_pi_ids_in_isolated_stores() {
        let root = tempfile::tempdir().unwrap();
        let c = root.path().join("claude");
        let p = root.path().join("pi");
        let claude = c.join("-fixture-project").join("session-id.jsonl");
        file(&claude);
        assert_eq!(
            resolve_with(
                &event(AgentKind::ClaudeCode, "session-id"),
                Some(c),
                None,
                None
            ),
            Some(claude)
        );
        let id = "b4371092-001c-44ba-a423-3d8e7b1d3bc0";
        let pi = p
            .join("--fixture-project--")
            .join(format!("2026-11-09T10-00-00-000Z_{id}.jsonl"));
        file(&pi);
        assert_eq!(
            resolve_with(&event(AgentKind::Pi, id), None, Some(p), None),
            Some(pi)
        );
    }
    #[test]
    fn resolves_native_flat_and_nested_qin_journals() {
        let root = tempfile::tempdir().unwrap();
        let flat = root.path().join("sessions/id.journal.jsonl");
        file(&flat);
        assert_eq!(
            resolve_with(
                &event(AgentKind::QinCode, "id"),
                None,
                None,
                Some(root.path().into())
            ),
            Some(flat)
        );
        let nested = root.path().join("sessions/other/journal.jsonl");
        file(&nested);
        assert_eq!(
            resolve_with(
                &event(AgentKind::QinCode, "other"),
                None,
                None,
                Some(root.path().into())
            ),
            Some(nested)
        );
    }
    #[test]
    fn ambiguous_ids_and_path_traversal_do_not_guess() {
        let root = tempfile::tempdir().unwrap();
        file(&root.path().join("a/id.jsonl"));
        file(&root.path().join("b/id.jsonl"));
        assert!(resolve_with(
            &event(AgentKind::ClaudeCode, "id"),
            Some(root.path().into()),
            None,
            None
        )
        .is_none());
        assert!(resolve_with(
            &event(AgentKind::ClaudeCode, "../id"),
            Some(root.path().into()),
            None,
            None
        )
        .is_none());
    }
}
