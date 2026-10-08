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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub self_dev: bool,
    /// Set only when codex ran against a `CODEX_HOME` other than the default:
    /// `codex resume` looks for the session there and nowhere else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_home: Option<String>,
    pub cwd: Option<String>,
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
        "qin-code" if !args.iter().skip(1).any(|arg| is_qin_non_client_arg(arg)) => {
            Some(LiveAgent::QinCode)
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
    now: SystemTime,
) -> Vec<LiveAgentSession> {
    let qin_claims = qin_argv_claims(found, qin_clients);
    let cwd_of = |f: &Found| {
        f.pids
            .iter()
            .find_map(|pid| open.get(pid).and_then(|files| files.cwd.clone()))
    };
    let mut pi_per_cwd: HashMap<Option<String>, usize> = HashMap::new();
    for f in found.iter().filter(|f| f.agent == LiveAgent::Pi) {
        *pi_per_cwd.entry(cwd_of(f)).or_default() += 1;
    }
    found
        .iter()
        .filter_map(|f| {
            let cwd = cwd_of(f);
            let mut session = LiveAgentSession {
                root_pid: f.root_pid,
                agent_id: f.agent.id(),
                session_id: String::new(),
                self_dev: false,
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
                    // Two pi processes in one directory write to the same folder
                    // and cannot be told apart from outside.
                    if pi_per_cwd.get(&cwd).copied().unwrap_or(0) > 1 {
                        return None;
                    }
                    let started = now.checked_sub(f.elapsed)?;
                    session.session_id =
                        pi_session(homes.pi_sessions.as_deref()?, cwd.as_deref()?, started)?;
                }
                // A client started inside qin-code's own repo turns self-dev on
                // by itself, so resuming from the session's directory restores
                // it; the argv flag covers a self-dev session started elsewhere.
                LiveAgent::QinCode => {
                    session.session_id = qin_session(f, cwd.as_deref(), qin_clients, &qin_claims)?;
                    session.self_dev = f.args.iter().any(|arg| arg == "self-dev");
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

/// pi records no pid and closes its session file after every append, so the
/// session is inferred: the file in this directory's session folder written
/// most recently since the process started. Nothing written yet means nothing
/// to resume.
fn pi_session(sessions_root: &Path, cwd: &str, started: SystemTime) -> Option<String> {
    let dir = sessions_root.join(encode_pi_project_dir(Path::new(cwd)));
    let since = started.checked_sub(Duration::from_secs(2))?;
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".jsonl")?;
            let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
            (modified >= since).then_some((modified, stem.to_string()))
        })
        .max_by_key(|(modified, _)| *modified)
        // `<ISO time>_<uuid>`; `pi --session <uuid>` finds it in the cwd folder.
        .and_then(|(_, stem)| safe_id(stem.rsplit('_').next()?))
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
    let Ok(output) = Command::new(crate::trackers::git_tracker::resolve_executable("qin-code"))
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
                self_dev: false,
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
    fn pi_takes_the_newest_session_written_since_it_started() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("pi")
            .join(encode_pi_project_dir(Path::new("/repo")));
        write_aged(
            &dir.join("2026-10-08T01-00-00-000Z_01a11a0a-0000-7000-8000-00000000000a.jsonl"),
            Duration::from_secs(3_600),
        );
        write_aged(
            &dir.join("2026-10-08T02-00-00-000Z_01a11a0b-0000-7000-8000-00000000000b.jsonl"),
            Duration::from_secs(10),
        );
        let open: HashMap<_, _> = [cwd_only(5, "/repo")].into();
        let resolved = resolve_all(
            &[found(LiveAgent::Pi, 5, "pi")],
            &open,
            &[],
            &homes(root.path()),
            SystemTime::now(),
        );
        assert_eq!(
            resolved[0].session_id,
            "01a11a0b-0000-7000-8000-00000000000b"
        );
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
        assert!(resolved[0].self_dev);
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
