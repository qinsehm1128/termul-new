//! Which CLI agent session is running under a terminal right now.
//!
//! Given the shell pids of open terminals, find a claude / codex / pi /
//! qin-code process below each one and work out the session it is in, so the
//! renderer can remember it and reopen it after the process is gone. Each agent
//! is resolved from the most direct evidence it leaves behind; when that
//! evidence is ambiguous the terminal is reported as unknown rather than
//! guessed. Metadata only: nothing here runs a resume command.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;

use super::paths::{encode_pi_project_dir, pi_sessions_dir, user_home};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAgentSession {
    pub root_pid: u32,
    pub agent_id: &'static str,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_dev: Option<bool>,
    /// Set only when codex ran against a `CODEX_HOME` other than the default:
    /// `codex resume` looks for the session there and nowhere else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_home: Option<String>,
    pub cwd: Option<String>,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none", rename = "hook_kind")]
    pub hook_kind: Option<crate::agent_hooks::HookKind>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "transcript_path")]
    pub transcript_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LiveAgent {
    Claude,
    Codex,
    Pi,
    QinCode,
}

impl LiveAgent {
    fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::QinCode => "qin-code",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Proc {
    pid: u32,
    ppid: u32,
    elapsed: Duration,
    args: Vec<String>,
}

/// An agent process found under a terminal, before its session is resolved.
#[derive(Debug, Clone)]
struct Found {
    root_pid: u32,
    agent: LiveAgent,
    /// The agent's own process and every descendant that is the same agent
    /// (codex runs a node wrapper above the native binary that holds its files).
    pids: Vec<u32>,
    args: Vec<String>,
    elapsed: Duration,
}

/// What `lsof` says about one process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OpenFiles {
    cwd: Option<String>,
    files: Vec<String>,
}

/// Active hook lifecycle state is authoritative. Unhooked or ended terminals
/// use the probe, but an ended hook identity cannot be resurrected by it.
pub fn detect_with_hooks(terminals: &[(u32, String, Option<String>)]) -> Vec<LiveAgentSession> {
    detect_with_hook_lookup(
        terminals,
        crate::agent_hooks::state::latest,
        detect_live_agent_sessions,
    )
}

fn detect_with_hook_lookup(
    terminals: &[(u32, String, Option<String>)],
    lookup: impl Fn(&str) -> Option<crate::agent_hooks::contract::HookStateDto>,
    poll: impl FnOnce(&[u32]) -> Vec<LiveAgentSession>,
) -> Vec<LiveAgentSession> {
    let mut fallback = Vec::new();
    let mut ended = std::collections::HashMap::new();
    let mut sessions = Vec::new();
    for (pid, id, cwd) in terminals {
        if let Some(event) = lookup(id) {
            if event.restore_state != crate::agent_hooks::contract::RestoreState::Ended {
                let self_dev = event
                    .identity
                    .descriptor_id
                    .as_deref()
                    .and_then(|id| {
                        let home = PathBuf::from(std::env::var_os("HOME")?);
                        super::identity_store::load(&home, id).ok()
                    })
                    .filter(|stored| {
                        stored.identity.agent_session_id == event.agent_session_id
                            && stored.adapter_install_id == event.adapter_install_id
                    })
                    .map(|stored| stored.variant == "self-dev");
                sessions.push(LiveAgentSession {
                    root_pid: *pid,
                    agent_id: match event.identity.adapter_id.as_str() {
                        "claude-code" => "claude-code",
                        "pi" => "pi",
                        "qin-code" => "qin-code",
                        _ => continue,
                    },
                    session_id: event.agent_session_id,
                    self_dev,
                    codex_home: None,
                    cwd: event.identity.cwd.or_else(|| cwd.clone()),
                    source: "hook",
                    hook_kind: None,
                    transcript_path: event.identity.transcript_path,
                });
            } else {
                // Uninstalling hooks must not disable discovery for the rest
                // of this PTY's life. Only suppress the ended identity.
                fallback.push(*pid);
                ended.insert(
                    *pid,
                    (
                        event.identity.adapter_id.as_str().to_string(),
                        event.agent_session_id,
                    ),
                );
            }
        } else {
            fallback.push(*pid);
        }
    }
    if !fallback.is_empty() {
        sessions.extend(poll(&fallback).into_iter().filter(|session| {
            !ended.get(&session.root_pid).is_some_and(|(agent, id)| {
                session.agent_id == *agent
                    && (session.session_id.is_empty() || session.session_id == *id)
            })
        }));
    }
    sessions
}

pub async fn terminal_hook_identities(
    service: &crate::core::TerminalServiceHandle,
) -> Vec<(u32, String, Option<String>)> {
    if let Some(client) = service.core_client() {
        return client
            .list()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.active)
            .map(|t| (t.pid, t.id, Some(t.cwd)))
            .collect();
    }
    service
        .in_process_pty()
        .map(|pty| {
            pty.get_all()
                .into_iter()
                .filter(|t| t.is_active())
                .map(|t| (t.pid, t.id.clone(), Some(t.cwd.clone())))
                .collect()
        })
        .unwrap_or_default()
}

pub fn detect_live_agent_sessions(root_pids: &[u32]) -> Vec<LiveAgentSession> {
    let procs = list_processes();
    let found: Vec<Found> = root_pids
        .iter()
        .filter_map(|&root| find_agent_under(&procs, root))
        .collect();
    if found.is_empty() {
        return Vec::new();
    }
    let open = open_files(found.iter().flat_map(|f| f.pids.iter().copied()));
    let qin_clients = if found.iter().any(|f| f.agent == LiveAgent::QinCode) {
        qin_clients_map()
    } else {
        Vec::new()
    };
    resolve_all(
        &found,
        &open,
        &qin_clients,
        &AgentHomes::from_env(),
        SystemTime::now(),
    )
}

// --- process table -------------------------------------------------------

/// All matching agent processes in the authenticated terminal ancestry, including direct launches.
/// Recording all matches is conservative: a surviving ambiguous/background agent blocks restore.
pub(crate) fn verified_agent_processes(root: u32, adapter: &str) -> Vec<u32> {
    let processes = list_processes();
    let mut queue: VecDeque<u32> = [root].into();
    let mut visited = HashSet::new();
    let mut result = Vec::new();
    while let Some(pid) = queue.pop_front() {
        if !visited.insert(pid) {
            continue;
        }
        if let Some(proc) = processes.iter().find(|proc| proc.pid == pid) {
            if classify(&proc.args).is_some_and(|agent| agent.id() == adapter) {
                result.push(pid);
            }
        }
        queue.extend(
            processes
                .iter()
                .filter(|proc| proc.ppid == pid)
                .map(|proc| proc.pid),
        );
    }
    result
}

pub(crate) fn qin_variant_from_evidence(
    launch: &[String],
    process: &[String],
) -> Option<&'static str> {
    fn self_dev(argv: &[String]) -> Option<bool> {
        if classify(argv)? != LiveAgent::QinCode {
            return None;
        }
        let offset = if matches!(basename(argv.first()?), "node" | "bun") {
            2
        } else {
            1
        };
        Some(argv.get(offset).is_some_and(|arg| arg == "self-dev"))
    }
    // A shell launch carries no qin variant evidence. The uniquely observed
    // qin child's own argv does; never infer this from the shared executable.
    let observed = self_dev(process)?;
    if classify(launch) == Some(LiveAgent::QinCode) && self_dev(launch)? != observed {
        return None;
    }
    Some(if observed { "self-dev" } else { "default" })
}
pub(crate) fn verified_qin_variant(root: u32, launch: &[String]) -> Option<String> {
    let pids = verified_agent_processes(root, "qin-code");
    if pids.is_empty() || (classify(launch) != Some(LiveAgent::QinCode) && pids.len() != 1) {
        return None;
    }
    let processes = list_processes();
    let mut result: Option<&str> = None;
    for pid in pids {
        let process = processes.iter().find(|process| process.pid == pid)?;
        let variant = qin_variant_from_evidence(launch, &process.args)?;
        if result.is_some_and(|previous| previous != variant) {
            return None;
        }
        result = Some(variant);
    }
    result.map(str::to_owned)
}

fn list_processes() -> Vec<Proc> {
    let Ok(output) = Command::new("ps")
        .args(["-axww", "-o", "pid=,ppid=,etime=,command="])
        .output()
    else {
        return Vec::new();
    };
    parse_ps(&String::from_utf8_lossy(&output.stdout))
}

fn parse_ps(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let elapsed = parse_etime(parts.next()?)?;
            let args: Vec<String> = parts.map(str::to_string).collect();
            (!args.is_empty()).then_some(Proc {
                pid,
                ppid,
                elapsed,
                args,
            })
        })
        .collect()
}

/// `ps` elapsed time: `[[dd-]hh:]mm:ss`.
fn parse_etime(text: &str) -> Option<Duration> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, text),
    };
    let fields: Vec<u64> = clock
        .split(':')
        .map(|field| field.parse().ok())
        .collect::<Option<_>>()?;
    let (hours, minutes, seconds) = match fields.as_slice() {
        [m, s] => (0, *m, *s),
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    Some(Duration::from_secs(
        ((days * 24 + hours) * 60 + minutes) * 60 + seconds,
    ))
}

fn basename(arg: &str) -> &str {
    arg.rsplit('/').next().unwrap_or(arg)
}

/// Which agent a command line is, if any. Node-hosted CLIs show up either
/// under their own process title (`pi`) or as `node <script>`.
fn classify(args: &[String]) -> Option<LiveAgent> {
    let program = basename(args.first()?);
    let script = args.get(1).map(|arg| basename(arg)).unwrap_or("");
    let name = if matches!(program, "node" | "bun") {
        script
    } else {
        program
    };
    match name {
        "claude" => Some(LiveAgent::Claude),
        "codex" => Some(LiveAgent::Codex),
        "pi" => Some(LiveAgent::Pi),
        // Only the TUI client is a session a terminal can reopen; the shared
        // server, the menubar helper and the web UI are not.
        "qin-code" => {
            let offset = if matches!(program, "node" | "bun") {
                2
            } else {
                1
            };
            let command = args.get(offset).map(String::as_str);
            let command = if command == Some("self-dev") {
                args.get(offset + 1).map(String::as_str)
            } else {
                command
            };
            // Session IDs and option values can equal subcommand names. Only
            // the actual command position determines whether this is a TUI.
            (!command.is_some_and(is_qin_non_client_arg)).then_some(LiveAgent::QinCode)
        }
        _ => None,
    }
}

fn is_qin_non_client_arg(arg: &str) -> bool {
    matches!(
        arg,
        "serve" | "menubar" | "setup-hotkey" | "--web" | "debug" | "restart" | "session"
    )
}

/// The agent closest to the terminal's shell. A nested agent (one launched by
/// another, e.g. codex as a tool of claude) belongs to the outer session.
fn find_agent_under(procs: &[Proc], root: u32) -> Option<Found> {
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for proc in procs {
        children.entry(proc.ppid).or_default().push(proc);
    }
    let mut queue: VecDeque<u32> = [root].into();
    let mut seen = HashSet::new();
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        for child in children.get(&pid).into_iter().flatten() {
            if let Some(agent) = classify(&child.args) {
                let mut pids = vec![child.pid];
                collect_same_agent(&children, child.pid, agent, &mut pids);
                return Some(Found {
                    root_pid: root,
                    agent,
                    pids,
                    args: child.args.clone(),
                    elapsed: child.elapsed,
                });
            }
            queue.push_back(child.pid);
        }
    }
    None
}

fn collect_same_agent(
    children: &HashMap<u32, Vec<&Proc>>,
    pid: u32,
    agent: LiveAgent,
    out: &mut Vec<u32>,
) {
    for child in children.get(&pid).into_iter().flatten() {
        if classify(&child.args) == Some(agent) {
            out.push(child.pid);
            collect_same_agent(children, child.pid, agent, out);
        }
    }
}

fn open_files(pids: impl Iterator<Item = u32>) -> HashMap<u32, OpenFiles> {
    let list: Vec<String> = pids.map(|pid| pid.to_string()).collect();
    if list.is_empty() {
        return HashMap::new();
    }
    let Ok(output) = Command::new("lsof")
        .args(["-n", "-P", "-Ffn", "-p", &list.join(",")])
        .output()
    else {
        return HashMap::new();
    };
    parse_lsof(&String::from_utf8_lossy(&output.stdout))
}

fn parse_lsof(text: &str) -> HashMap<u32, OpenFiles> {
    let mut out: HashMap<u32, OpenFiles> = HashMap::new();
    let mut pid = None;
    let mut fd = String::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().ok();
        } else if let Some(value) = line.strip_prefix('f') {
            fd = value.to_string();
        } else if let (Some(pid), Some(name)) = (pid, line.strip_prefix('n')) {
            let entry = out.entry(pid).or_default();
            if fd == "cwd" {
                entry.cwd = Some(name.to_string());
            } else {
                entry.files.push(name.to_string());
            }
        }
    }
    out
}

// --- resolution ----------------------------------------------------------

struct AgentHomes {
    claude: Option<PathBuf>,
    default_codex: Option<PathBuf>,
    pi_sessions: Option<PathBuf>,
}

impl AgentHomes {
    fn from_env() -> Self {
        Self {
            claude: std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .or_else(|| user_home().map(|h| h.join(".claude"))),
            default_codex: user_home().map(|h| h.join(".codex")),
            pi_sessions: pi_sessions_dir(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QinClient {
    session_id: String,
    working_dir: Option<String>,
}

fn resolve_all(
    found: &[Found],
    open: &HashMap<u32, OpenFiles>,
    qin_clients: &[QinClient],
    homes: &AgentHomes,
    _now: SystemTime,
) -> Vec<LiveAgentSession> {
    let qin_claims = qin_argv_claims(found, qin_clients);
    let cwd_of = |f: &Found| {
        f.pids
            .iter()
            .find_map(|pid| open.get(pid).and_then(|files| files.cwd.clone()))
    };
    found
        .iter()
        .filter_map(|f| {
            let cwd = cwd_of(f);
            let mut session = LiveAgentSession {
                root_pid: f.root_pid,
                agent_id: f.agent.id(),
                session_id: String::new(),
                self_dev: Some(false),
                source: "poll",
                hook_kind: None,
                transcript_path: None,
                codex_home: None,
                cwd: cwd.clone(),
            };
            match f.agent {
                LiveAgent::Claude => {
                    session.session_id = claude_session(homes.claude.as_deref()?, &f.pids)?;
                }
                LiveAgent::Codex => {
                    let files = f
                        .pids
                        .iter()
                        .filter_map(|pid| open.get(pid))
                        .flat_map(|files| files.files.iter().map(String::as_str));
                    let (id, home) = codex_session(files)?;
                    session.session_id = id;
                    if Some(&home) != homes.default_codex.as_ref() {
                        session.codex_home = Some(home.to_string_lossy().into_owned());
                    }
                }
                LiveAgent::Pi => {
                    let files = f
                        .pids
                        .iter()
                        .filter_map(|pid| open.get(pid))
                        .flat_map(|info| info.files.iter().map(String::as_str));
                    let (id, path) = pi_process_session(
                        homes.pi_sessions.as_deref()?,
                        cwd.as_deref()?,
                        &f.args,
                        files,
                    )?;
                    session.session_id = id;
                    session.transcript_path = Some(path.to_string_lossy().into_owned());
                }
                // A client started inside qin-code's own repo turns self-dev on
                // by itself, so resuming from the session's directory restores
                // it; the argv flag covers a self-dev session started elsewhere.
                LiveAgent::QinCode => {
                    session.session_id = qin_session(f, cwd.as_deref(), qin_clients, &qin_claims)?;
                    session.self_dev = Some(f.args.iter().any(|arg| arg == "self-dev"));
                }
            }
            Some(session)
        })
        .collect()
}

fn safe_id(id: &str) -> Option<String> {
    let ok = !id.is_empty()
        && id.len() <= super::types::SESSION_ID_MAX_LEN
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        && !id.starts_with('-');
    ok.then(|| id.to_string())
}

/// Claude Code keeps `<config>/sessions/<pid>.json` up to date with the
/// session that process is in, including after `/clear` or `/resume`.
fn claude_session(claude_home: &Path, pids: &[u32]) -> Option<String> {
    pids.iter().find_map(|pid| {
        let text =
            std::fs::read_to_string(claude_home.join("sessions").join(format!("{pid}.json")))
                .ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        safe_id(json.get("sessionId")?.as_str()?)
    })
}

/// Codex holds every rollout it has loaded open for writing
/// (`<home>/sessions/YYYY/MM/DD/rollout-<time>-<uuid>.jsonl`). After `/new` or
/// `/resume` it holds several; the one written last is the one in use. A
/// session with no message yet has no rollout and nothing to resume.
fn codex_session<'a>(files: impl Iterator<Item = &'a str>) -> Option<(String, PathBuf)> {
    files
        .filter_map(|name| {
            let path = Path::new(name);
            let file = path.file_name()?.to_str()?;
            let stem = file.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
            let home = PathBuf::from(&name[..name.find("/sessions/")?]);
            // The id is the trailing UUID: 36 characters after the timestamp.
            let id = stem.get(stem.len().checked_sub(36)?..)?;
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
            Some((modified, safe_id(id)?, home))
        })
        .max_by_key(|(modified, _, _)| *modified)
        .map(|(_, id, home)| (id, home))
}

/// Resolve only process-evidenced handles. Never select a cwd's newest file.
fn pi_process_session<'a>(
    root: &Path,
    cwd: &str,
    args: &[String],
    files: impl Iterator<Item = &'a str>,
) -> Option<(String, PathBuf)> {
    let mut candidates: Vec<PathBuf> = files
        .filter_map(|name| safe_pi_path(root, Path::new(name)))
        .collect();
    if let Some(handle) = arg_after(args, "--session") {
        let path = Path::new(handle);
        if path.is_absolute() {
            candidates.push(safe_pi_path(root, path)?);
        } else if safe_id(handle).is_some() {
            let dir = root.join(encode_pi_project_dir(Path::new(cwd)));
            let matching: Vec<_> = std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .filter_map(|entry| {
                    let path = safe_pi_path(root, &entry.path())?;
                    (pi_path_id(&path)?.as_str() == handle).then_some(path)
                })
                .collect();
            if matching.len() != 1 {
                return None;
            }
            candidates.extend(matching);
        } else {
            return None;
        }
    }
    candidates.sort();
    candidates.dedup();
    if candidates.len() != 1 {
        return None;
    }
    let path = candidates.pop()?;
    Some((pi_path_id(&path)?, path))
}
pub(crate) fn safe_pi_path(root: &Path, path: &Path) -> Option<PathBuf> {
    if !path.is_absolute()
        || super::identity_store::safe_path(path).is_err()
        || super::identity_store::safe_path(root).is_err()
    {
        return None;
    }
    let canonical_root = root.canonicalize().ok()?;
    let canonical = path.canonicalize().ok()?;
    if !canonical.starts_with(canonical_root)
        || !canonical.is_file()
        || canonical.extension()?.to_str()? != "jsonl"
    {
        return None;
    }
    std::fs::File::open(&canonical).ok()?;
    Some(canonical)
}
fn pi_path_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    safe_id(stem.rsplit('_').next()?)
}

fn arg_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

/// Session ids that qin-code clients' own argv names *and* the server confirms
/// is live. A client re-execs with `--resume <current id>` on every reload, but
/// `/clear` or the session picker switch sessions in place, leaving the argv
/// stale — so the server's client table is the authority.
fn qin_argv_claims(found: &[Found], clients: &[QinClient]) -> HashSet<String> {
    found
        .iter()
        .filter(|f| f.agent == LiveAgent::QinCode)
        .filter_map(|f| arg_after(&f.args, "--resume"))
        .filter(|id| clients.iter().any(|client| client.session_id == *id))
        .map(str::to_string)
        .collect()
}

/// qin-code clients record no pid anywhere, so: trust the argv id when the
/// server confirms it; otherwise take the one live session in this client's
/// directory that no other client's argv accounts for. More than one such
/// session cannot be told apart, and is left unknown.
fn qin_session(
    found: &Found,
    cwd: Option<&str>,
    clients: &[QinClient],
    claims: &HashSet<String>,
) -> Option<String> {
    if let Some(id) = arg_after(&found.args, "--resume") {
        if clients.iter().any(|client| client.session_id == id) {
            return safe_id(id);
        }
    }
    let cwd = cwd?;
    let mut candidates = clients.iter().filter(|client| {
        !claims.contains(&client.session_id) && client.working_dir.as_deref() == Some(cwd)
    });
    let only = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    safe_id(&only.session_id)
}

fn qin_clients_map() -> Vec<QinClient> {
    // The app's own PATH (from launchd) rarely has the user's bin dirs; look
    // the command up on the login shell's PATH, as terminals do.
    let mut env = HashMap::new();
    crate::pty::env_refresh::apply_fresh_path(&mut env);
    let mut command = Command::new("qin-code");
    if let Some(path) = env.get("PATH") {
        command.env("PATH", path);
    }
    let Ok(output) = command
        .args(["debug", "clients:map"])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_qin_clients(&String::from_utf8_lossy(&output.stdout))
}

fn parse_qin_clients(text: &str) -> Vec<QinClient> {
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    json.get("clients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|client| {
            Some(QinClient {
                session_id: client.get("session_id")?.as_str()?.to_string(),
                working_dir: client
                    .get("working_dir")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect()
}

#[cfg(test)]
mod hook_precedence_tests {
    use super::*;
    use crate::agent_hooks::{contract::*, HookKind};
    fn event(kind: HookKind) -> HookStateDto {
        let mut value = crate::cli_session::restore_tests::state();
        value.terminal_id = "hooked".into();
        value.identity.adapter_id = "qin-code".to_owned().try_into().unwrap();
        value.identity.agent_session_id = "actual-session".into();
        value.agent_session_id = "actual-session".into();
        value.restore_state = if kind == HookKind::SessionEnd {
            RestoreState::Ended
        } else {
            RestoreState::Confirmed
        };
        value
    }
    #[test]
    fn hook_state_precedes_polling_and_only_unhooked_roots_are_probed() {
        let terminals = [
            (10, "hooked".into(), Some("/project".into())),
            (20, "plain".into(), None),
        ];
        let sessions = detect_with_hook_lookup(
            &terminals,
            |id| (id == "hooked").then(|| event(HookKind::NeedsApproval)),
            |roots| {
                assert_eq!(roots, &[20]);
                vec![]
            },
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "actual-session");
        assert_eq!(sessions[0].source, "hook");
        assert_eq!(sessions[0].self_dev, None);
        assert!(serde_json::to_value(&sessions[0])
            .unwrap()
            .get("selfDev")
            .is_none());
        assert_eq!(sessions[0].hook_kind, None);
    }
    #[test]
    fn same_directory_hooks_keep_qin_and_pi_sessions_distinct() {
        let terminals: Vec<_> = (0..4)
            .map(|n| (10 + n, format!("pane-{n}"), Some("/same-project".into())))
            .collect();
        let sessions = detect_with_hook_lookup(
            &terminals,
            |id| {
                let mut value = event(HookKind::SessionStart);
                value.terminal_id = id.into();
                value.agent_session_id = format!("session-{id}");
                value.identity.agent_session_id = value.agent_session_id.clone();
                value.identity.adapter_id = if id.ends_with('0') || id.ends_with('1') {
                    "qin-code"
                } else {
                    "pi"
                }
                .to_owned()
                .try_into()
                .unwrap();
                Some(value)
            },
            |_| panic!("active hook panes must not poll"),
        );
        assert_eq!(sessions.len(), 4);
        assert_eq!(
            sessions
                .iter()
                .map(|s| &s.session_id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            4
        );
        assert_eq!(
            sessions.iter().filter(|s| s.agent_id == "qin-code").count(),
            2
        );
        assert_eq!(sessions.iter().filter(|s| s.agent_id == "pi").count(), 2);
    }

    fn polled(agent: &'static str, session: &str) -> LiveAgentSession {
        LiveAgentSession {
            root_pid: 10,
            agent_id: agent,
            session_id: session.into(),
            self_dev: Some(false),
            codex_home: None,
            cwd: None,
            source: "poll",
            hook_kind: None,
            transcript_path: None,
        }
    }
    #[test]
    fn ended_hook_session_is_not_resurrected_by_a_stale_probe() {
        let terminals = [(10, "hooked".into(), None)];
        let sessions = detect_with_hook_lookup(
            &terminals,
            |_| Some(event(HookKind::SessionEnd)),
            |_| vec![polled("qin-code", "actual-session"), polled("qin-code", "")],
        );
        assert!(sessions.is_empty());
    }
    #[test]
    fn ended_hook_does_not_disable_future_agents_after_uninstall() {
        let terminals = [(10, "hooked".into(), None)];
        let sessions = detect_with_hook_lookup(
            &terminals,
            |_| Some(event(HookKind::SessionEnd)),
            |_| vec![polled("codex", "new-codex"), polled("qin-code", "new-qin")],
        );
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().all(|session| session.source == "poll"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &str) -> Vec<String> {
        cmd.split_whitespace().map(str::to_string).collect()
    }

    fn proc(pid: u32, ppid: u32, cmd: &str) -> Proc {
        Proc {
            pid,
            ppid,
            elapsed: Duration::from_secs(60),
            args: args(cmd),
        }
    }

    fn found(agent: LiveAgent, pid: u32, cmd: &str) -> Found {
        Found {
            root_pid: pid,
            agent,
            pids: vec![pid],
            args: args(cmd),
            elapsed: Duration::from_secs(60),
        }
    }

    fn homes(root: &Path) -> AgentHomes {
        AgentHomes {
            claude: Some(root.join("claude")),
            default_codex: Some(root.join("codex")),
            pi_sessions: Some(root.join("pi")),
        }
    }

    fn cwd_only(pid: u32, cwd: &str) -> (u32, OpenFiles) {
        (
            pid,
            OpenFiles {
                cwd: Some(cwd.to_string()),
                files: Vec::new(),
            },
        )
    }

    #[test]
    fn parse_ps_reads_pid_ppid_elapsed_and_argv() {
        let procs = parse_ps(
            "  10     1 1-02:03:04 /bin/zsh -l\n  11    10 05:06 claude --resume abc\n\nbad\n",
        );
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].elapsed, Duration::from_secs(93_784));
        assert_eq!(procs[1].elapsed, Duration::from_secs(306));
        assert_eq!(procs[1].args, args("claude --resume abc"));
        assert_eq!(procs[1].ppid, 10);
    }

    #[test]
    fn classify_recognises_the_four_agents_and_skips_qin_helpers() {
        let c = |cmd: &str| classify(&args(cmd));
        assert_eq!(
            c("claude --dangerously-skip-permissions"),
            Some(LiveAgent::Claude)
        );
        assert_eq!(c("node /Users/me/.bun/bin/codex"), Some(LiveAgent::Codex));
        assert_eq!(
            c("/x/vendor/aarch64-apple-darwin/bin/codex"),
            Some(LiveAgent::Codex)
        );
        assert_eq!(c("pi"), Some(LiveAgent::Pi));
        assert_eq!(
            c("/u/.qin-code/builds/current/qin-code self-dev --resume s1"),
            Some(LiveAgent::QinCode)
        );
        assert_eq!(c("qin-code --web"), None);
        assert_eq!(
            c("/u/.qin-code/builds/shared-server/qin-code serve --socket x"),
            None
        );
        assert_eq!(c("vim claude.md"), None);
        assert_eq!(c("node server.js"), None);
    }

    #[test]
    fn finds_the_outermost_agent_and_its_same_agent_children() {
        let procs = vec![
            proc(10, 1, "-zsh"),
            proc(20, 10, "node /u/.bun/bin/codex"),
            proc(21, 20, "/u/vendor/bin/codex"),
            proc(30, 21, "claude -p hi"),
            proc(40, 1, "-zsh"),
            proc(41, 40, "vim"),
        ];
        let found = find_agent_under(&procs, 10).expect("codex under 10");
        assert_eq!(found.agent, LiveAgent::Codex);
        assert_eq!(found.pids, vec![20, 21]);
        assert!(find_agent_under(&procs, 40).is_none());
    }

    #[test]
    fn parse_lsof_separates_cwd_from_open_files() {
        let map = parse_lsof(
            "p11\nfcwd\nn/repo\nf41\nn/h/.codex/sessions/a.jsonl\np12\nfcwd\nn/other dir\n",
        );
        assert_eq!(map[&11].cwd.as_deref(), Some("/repo"));
        assert_eq!(
            map[&11].files,
            vec!["/h/.codex/sessions/a.jsonl".to_string()]
        );
        assert_eq!(map[&12].cwd.as_deref(), Some("/other dir"));
    }

    #[test]
    fn claude_session_reads_the_pid_file_and_rejects_unsafe_ids() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("claude").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("77.json"),
            r#"{"pid":77,"sessionId":"ca50b276-5627-4a6a-8fea-ce2bfbe86c3d","cwd":"/repo"}"#,
        )
        .unwrap();
        std::fs::write(sessions.join("78.json"), r#"{"sessionId":"x; rm -rf ~"}"#).unwrap();
        let open: HashMap<_, _> = [cwd_only(77, "/repo")].into();
        let resolved = resolve_all(
            &[
                found(LiveAgent::Claude, 77, "claude"),
                found(LiveAgent::Claude, 78, "claude"),
            ],
            &open,
            &[],
            &homes(root.path()),
            SystemTime::now(),
        );
        assert_eq!(
            resolved,
            vec![LiveAgentSession {
                root_pid: 77,
                agent_id: "claude-code",
                session_id: "ca50b276-5627-4a6a-8fea-ce2bfbe86c3d".into(),
                self_dev: Some(false),
                source: "poll",
                hook_kind: None,
                transcript_path: None,
                codex_home: None,
                cwd: Some("/repo".into()),
            }]
        );
    }

    fn write_aged(path: &Path, age: Duration) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "{}\n").unwrap();
        let when = SystemTime::now() - age;
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn codex_takes_the_rollout_written_last_and_its_home() {
        let root = tempfile::tempdir().unwrap();
        let day = root.path().join("codex").join("sessions/2026/10/08");
        let old =
            day.join("rollout-2026-10-08T10-00-00-01a0849d-0000-7000-8000-000000000001.jsonl");
        let new =
            day.join("rollout-2026-10-08T11-00-00-01a0849d-0000-7000-8000-000000000002.jsonl");
        write_aged(&old, Duration::from_secs(600));
        write_aged(&new, Duration::from_secs(5));
        let files = [
            "/dev/ttys001".to_string(),
            old.to_string_lossy().into_owned(),
            new.to_string_lossy().into_owned(),
        ];
        let (id, home) = codex_session(files.iter().map(String::as_str)).unwrap();
        assert_eq!(id, "01a0849d-0000-7000-8000-000000000002");
        assert_eq!(home, root.path().join("codex"));
        assert!(codex_session(["/dev/ttys001"].into_iter()).is_none());
    }

    #[test]
    fn codex_reports_a_non_default_home_only() {
        let root = tempfile::tempdir().unwrap();
        let rollout = root
            .path()
            .join("elsewhere/sessions/2026/10/08/rollout-2026-10-08T11-00-00-01a0849d-0000-7000-8000-000000000002.jsonl");
        write_aged(&rollout, Duration::from_secs(1));
        let open: HashMap<_, _> = [(
            21,
            OpenFiles {
                cwd: Some("/repo".into()),
                files: vec![rollout.to_string_lossy().into_owned()],
            },
        )]
        .into();
        let mut f = found(LiveAgent::Codex, 20, "node codex");
        f.pids = vec![20, 21];
        let resolved = resolve_all(&[f], &open, &[], &homes(root.path()), SystemTime::now());
        assert_eq!(
            resolved[0].session_id,
            "01a0849d-0000-7000-8000-000000000002"
        );
        assert_eq!(
            resolved[0].codex_home.as_deref(),
            Some(root.path().join("elsewhere").to_string_lossy().as_ref())
        );
        assert_eq!(resolved[0].cwd.as_deref(), Some("/repo"));
    }

    #[test]
    fn pi_uses_exact_process_file_or_unique_explicit_id_never_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let dir = root.join(encode_pi_project_dir(Path::new("/work")));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("old_session-one.jsonl");
        let newest = dir.join("new_session-two.jsonl");
        std::fs::write(&first, "{}").unwrap();
        std::fs::write(&newest, "{}").unwrap();
        assert!(pi_process_session(&root, "/work", &[], std::iter::empty()).is_none());
        let exact = pi_process_session(
            &root,
            "/work",
            &[],
            std::iter::once(first.to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(exact.0, "session-one");
        assert_eq!(exact.1, first);
        let args = args("--session session-one");
        assert_eq!(
            pi_process_session(&root, "/work", &args, std::iter::empty())
                .unwrap()
                .1,
            first
        );
        std::fs::write(dir.join("duplicate_session-one.jsonl"), "{}").unwrap();
        assert!(pi_process_session(&root, "/work", &args, std::iter::empty()).is_none());
    }

    #[test]
    fn pi_with_nothing_written_since_start_has_no_session() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("pi")
            .join(encode_pi_project_dir(Path::new("/repo")));
        write_aged(
            &dir.join("2026-10-08T01-00-00-000Z_01a11a0a-0000-7000-8000-00000000000a.jsonl"),
            Duration::from_secs(3_600),
        );
        let open: HashMap<_, _> = [cwd_only(5, "/repo")].into();
        // Started a minute ago; the only file predates it.
        let resolved = resolve_all(
            &[found(LiveAgent::Pi, 5, "pi")],
            &open,
            &[],
            &homes(root.path()),
            SystemTime::now(),
        );
        assert!(resolved.is_empty());
    }

    #[test]
    fn two_pi_processes_in_one_directory_stay_unknown() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("pi")
            .join(encode_pi_project_dir(Path::new("/repo")));
        write_aged(
            &dir.join("2026-10-08T02-00-00-000Z_01a11a0b-0000-7000-8000-00000000000b.jsonl"),
            Duration::from_secs(10),
        );
        let open: HashMap<_, _> = [cwd_only(5, "/repo"), cwd_only(6, "/repo")].into();
        let resolved = resolve_all(
            &[found(LiveAgent::Pi, 5, "pi"), found(LiveAgent::Pi, 6, "pi")],
            &open,
            &[],
            &homes(root.path()),
            SystemTime::now(),
        );
        assert!(resolved.is_empty());
    }

    fn client(id: &str, dir: &str) -> QinClient {
        QinClient {
            session_id: id.to_string(),
            working_dir: Some(dir.to_string()),
        }
    }

    #[test]
    fn qin_trusts_an_argv_id_the_server_confirms() {
        let clients = vec![client("s1", "/a"), client("s2", "/a")];
        let f = found(LiveAgent::QinCode, 1, "qin-code self-dev --resume s2");
        let open: HashMap<_, _> = [cwd_only(1, "/a")].into();
        let resolved = resolve_all(
            std::slice::from_ref(&f),
            &open,
            &clients,
            &homes(Path::new("/nonexistent")),
            SystemTime::now(),
        );
        assert_eq!(resolved[0].session_id, "s2");
        assert_eq!(resolved[0].self_dev, Some(true));
    }

    #[test]
    fn qin_falls_back_to_the_one_unclaimed_session_in_its_directory() {
        // f1's argv is stale (it /clear-ed into s3); f2 still owns s1.
        let clients = vec![client("s1", "/a"), client("s3", "/a"), client("s9", "/b")];
        let f1 = found(LiveAgent::QinCode, 1, "qin-code --resume s-old");
        let f2 = found(LiveAgent::QinCode, 2, "qin-code --resume s1");
        let claims = qin_argv_claims(&[f1.clone(), f2], &clients);
        assert_eq!(
            qin_session(&f1, Some("/a"), &clients, &claims),
            Some("s3".into())
        );
    }

    #[test]
    fn qin_leaves_an_ambiguous_client_unknown() {
        let clients = vec![client("s1", "/a"), client("s2", "/a")];
        let f = found(LiveAgent::QinCode, 1, "qin-code");
        let claims = qin_argv_claims(std::slice::from_ref(&f), &clients);
        assert_eq!(qin_session(&f, Some("/a"), &clients, &claims), None);
    }

    #[test]
    fn qin_rejects_an_argv_id_the_server_does_not_know() {
        let clients = vec![client("s1", "/b")];
        let f = found(LiveAgent::QinCode, 1, "qin-code --resume s-gone");
        let claims = qin_argv_claims(std::slice::from_ref(&f), &clients);
        assert_eq!(qin_session(&f, Some("/a"), &clients, &claims), None);
    }

    #[test]
    fn parse_qin_clients_reads_the_debug_map() {
        let clients = parse_qin_clients(
            r#"{"clients":[{"client_id":"c1","session_id":"session_kitten_1","working_dir":"/repo","status":"ready"}],"count":1}"#,
        );
        assert_eq!(clients, vec![client("session_kitten_1", "/repo")]);
        assert!(parse_qin_clients("not json").is_empty());
    }
}
