//! Child side of the host-injected plan tool.
//!
//! Entered via the hidden `--internal-mcp-plan-server` subcommand (both the
//! desktop `se-manager` and standalone `se-server` binaries branch on
//! this flag in `main` / `server_main` BEFORE any Tauri/AppHandle setup). The
//! agent spawns `current_exe() --internal-mcp-plan-server` as the injected
//! `McpServer::Stdio`; the child inherits the agent-provided stdin/stdout (the
//! MCP stdio transport).
//!
//! The child runs an rmcp MCP SERVER over stdio exposing the `plan`
//! tool. On each `tools/call`, it opens a fresh TCP connection to the parent
//! (port + token from env), forwards the input, and returns the parent's reply
//! to the agent. Minimal runtime: no Tauri plugins, no `AppHandle`, no sinks —
//! works identically on desktop + standalone.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::service::serve_server;
use rmcp::{tool, tool_router};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::acp::host_mcp::{
    FrameKind, FrameReply, FrameRequest, ScheduledTaskDraftCreateInput,
    ScheduledTaskDraftUpdateInput, ScheduledTaskGetInput, ScheduledTaskListInput,
    ScheduledTaskPauseInput, ScheduledTaskPreviewInput, SePlanInput, SeSetTitleInput, ENV_AGENT_ID,
    ENV_PORT, ENV_SESSION_ID, ENV_TOKEN,
};

/// Env-derived configuration for the child. Extracted so the arg parser is
/// unit-testable without touching `std::env`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildConfig {
    pub port: u16,
    pub token: String,
    pub session_id: String,
    pub agent_id: String,
}

/// Parse the child's env (`SE_PLAN_PORT` / `_TOKEN` / `_SESSION_ID` /
/// `_AGENT_ID`). Returns an error string (not an enum) so `run()` can print it
/// verbatim to stderr + exit 1 — matching the matrix's "child exits non-zero
/// within 5s" AC.
pub fn parse_env() -> Result<ChildConfig, String> {
    let port: u16 = std::env::var(ENV_PORT)
        .ok()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("missing or invalid {ENV_PORT}"))?;
    let token = std::env::var(ENV_TOKEN)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("missing {ENV_TOKEN}"))?;
    let session_id = std::env::var(ENV_SESSION_ID)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("missing {ENV_SESSION_ID}"))?;
    // AGENT_ID is optional (used only for logging in the parent); absent → "".
    let agent_id = std::env::var(ENV_AGENT_ID).unwrap_or_default();
    Ok(ChildConfig {
        port,
        token,
        session_id,
        agent_id,
    })
}

/// Subcommand entrypoint. Called from `main.rs` (desktop) / `server_main.rs`
/// (standalone) when the first arg is `--internal-mcp-plan-server`. Returns an
/// `i32` exit code so both call sites can use it (`std::process::exit` on
/// desktop, `ExitCode::from(code as u8)` on standalone). Never returns
/// normally on failure — prints an error to stderr + returns non-zero.
pub fn run() -> i32 {
    let config = match parse_env() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("[host-mcp-child] {msg}");
            return 1;
        }
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[host-mcp-child] failed to start runtime: {e}");
            return 1;
        }
    };

    match runtime.block_on(serve_mcp_server(config)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("[host-mcp-child] {e}");
            1
        }
    }
}

/// The rmcp MCP server service backing `plan`. Holds the per-session
/// connection info (port + token + session_id) so each `tools/call` can open a
/// fresh TCP connection to the parent.
struct SePlanServer {
    config: ChildConfig,
}

/// rmcp derives the `tools/list` entry from the `#[tool]` attribute; the input
/// type must implement `schemars::JsonSchema` so rmcp can generate the
/// `inputSchema`. We re-export the shared `SePlanInput` (defined in
/// `host_mcp::mod`) — it already derives `JsonSchema`.
///
/// `server_handler` on `#[tool_router]` auto-generates the `ServerHandler` impl
/// (no separate `impl ServerHandler` block needed — adding one would duplicate
/// the impl + fail to compile).
#[tool_router(server_handler)]
impl SePlanServer {
    #[tool(
        name = "plan",
        description = "Update the execution plan / todo list shown in the Se plan panel. You MUST call this instead of any built-in todo/task tool — do not maintain your own todo list. Every time you would create or update a task, call this tool so the user sees a unified plan UI across all agents."
    )]
    async fn plan(&self, Parameters(input): Parameters<SePlanInput>) -> String {
        let request = FrameRequest {
            token: self.config.token.clone(),
            session_id: self.config.session_id.clone(),
            kind: FrameKind::Plan,
            todos: input.todos,
            title: None,
            payload: None,
        };
        match forward_to_parent(&self.config, request, "plan updated").await {
            Ok(msg) => msg,
            Err(e) => format!("plan error: {e}"),
        }
    }

    #[tool(
        name = "set_session_title",
        description = "Set a concise title for the current Se chat session. Call this EXACTLY ONCE per session, during the first turn, as soon as the user's intent is clear. Do not call it again for the same session — subsequent calls are ignored."
    )]
    async fn set_session_title(&self, Parameters(input): Parameters<SeSetTitleInput>) -> String {
        let request = FrameRequest {
            token: self.config.token.clone(),
            session_id: self.config.session_id.clone(),
            kind: FrameKind::SetTitle,
            todos: Vec::new(),
            title: Some(input.title),
            payload: None,
        };
        match forward_to_parent(&self.config, request, "title updated").await {
            Ok(msg) => msg,
            Err(e) => format!("set_session_title error: {e}"),
        }
    }

    #[tool(
        name = "scheduled_task_list",
        description = "List Se scheduled tasks across Conversations and optional project associations. This is read-only."
    )]
    async fn scheduled_task_list(
        &self,
        Parameters(input): Parameters<ScheduledTaskListInput>,
    ) -> String {
        self.forward_scheduled(FrameKind::ScheduledTaskList, input, "tasks listed")
            .await
    }

    #[tool(
        name = "scheduled_task_get",
        description = "Get one Se scheduled task and its current review revision/hash. This is read-only."
    )]
    async fn scheduled_task_get(
        &self,
        Parameters(input): Parameters<ScheduledTaskGetInput>,
    ) -> String {
        self.forward_scheduled(FrameKind::ScheduledTaskGet, input, "task loaded")
            .await
    }

    #[tool(
        name = "scheduled_task_preview",
        description = "Validate and normalize a cron, interval, or one-time schedule and return future executions. Always call this before creating a draft; the host is authoritative for timezone and DST behavior."
    )]
    async fn scheduled_task_preview(
        &self,
        Parameters(input): Parameters<ScheduledTaskPreviewInput>,
    ) -> String {
        self.forward_scheduled(FrameKind::ScheduledTaskPreview, input, "schedule previewed")
            .await
    }

    #[tool(
        name = "scheduled_task_draft_create",
        description = "Create a review-only scheduled task draft. This never activates or runs the task. Do not include secrets; explain the draft and wait for explicit user confirmation in Se."
    )]
    async fn scheduled_task_draft_create(
        &self,
        Parameters(input): Parameters<ScheduledTaskDraftCreateInput>,
    ) -> String {
        self.forward_scheduled(
            FrameKind::ScheduledTaskDraftCreate,
            input,
            "task draft created",
        )
        .await
    }

    #[tool(
        name = "scheduled_task_draft_update",
        description = "Replace an existing review-only draft using its expected revision. This never activates the task."
    )]
    async fn scheduled_task_draft_update(
        &self,
        Parameters(input): Parameters<ScheduledTaskDraftUpdateInput>,
    ) -> String {
        self.forward_scheduled(
            FrameKind::ScheduledTaskDraftUpdate,
            input,
            "task draft updated",
        )
        .await
    }

    #[tool(
        name = "scheduled_task_pause",
        description = "Safely pause future occurrences of a scheduled task using revision CAS. An already-started Conversation is not cancelled."
    )]
    async fn scheduled_task_pause(
        &self,
        Parameters(input): Parameters<ScheduledTaskPauseInput>,
    ) -> String {
        self.forward_scheduled(FrameKind::ScheduledTaskPause, input, "task paused")
            .await
    }

    async fn forward_scheduled<T: serde::Serialize>(
        &self,
        kind: FrameKind,
        input: T,
        success_message: &'static str,
    ) -> String {
        let payload = match serde_json::to_value(input) {
            Ok(payload) => payload,
            Err(error) => return format!("scheduled task input error: {error}"),
        };
        let request = FrameRequest {
            token: self.config.token.clone(),
            session_id: self.config.session_id.clone(),
            kind,
            todos: Vec::new(),
            title: None,
            payload: Some(payload),
        };
        match forward_to_parent(&self.config, request, success_message).await {
            Ok(message) => message,
            Err(error) => format!("scheduled task error: {error}"),
        }
    }
}

/// Connect to the parent TCP listener, send one frame, read one reply.
/// Fresh connection per call (localhost, sub-ms) — simplest + most robust.
/// The whole round trip is bounded so a wedged parent can't hang the agent's
/// tool call indefinitely.
async fn forward_to_parent(
    config: &ChildConfig,
    request: FrameRequest,
    success_message: &'static str,
) -> Result<String, String> {
    // 10s covers a healthy round trip many times over; a parent that can't
    // reply by then is wedged and the agent deserves a clear timeout error.
    const ROUND_TRIP: std::time::Duration = std::time::Duration::from_secs(10);
    tokio::time::timeout(
        ROUND_TRIP,
        forward_to_parent_inner(config, request, success_message),
    )
    .await
    .map_err(|_| "parent round trip timed out".to_string())?
}

async fn forward_to_parent_inner(
    config: &ChildConfig,
    request: FrameRequest,
    success_message: &'static str,
) -> Result<String, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", config.port))
        .await
        .map_err(|e| format!("connect to parent failed: {e}"))?;
    let mut buf = serde_json::to_vec(&request).map_err(|e| format!("encode frame: {e}"))?;
    buf.push(b'\n');
    stream
        .write_all(&buf)
        .await
        .map_err(|e| format!("write frame: {e}"))?;

    const MAX_REPLY: u64 = 64 * 1024;
    let mut reader = BufReader::new(stream.take(MAX_REPLY));
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| format!("read reply: {e}"))?;
    let reply: FrameReply =
        serde_json::from_str(&line).map_err(|e| format!("decode reply: {e}"))?;
    if reply.ok {
        match reply.result {
            Some(result) => serde_json::to_string_pretty(&result)
                .map_err(|error| format!("encode parent result: {error}")),
            None => Ok(success_message.to_string()),
        }
    } else {
        Err(reply.error.unwrap_or_else(|| "unknown error".to_string()))
    }
}

/// How often the watchdog asks the parent whether this child still has a job.
const PARENT_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Consecutive failed probes before the child concludes it is useless.
/// Consecutive, not cumulative: a parent that is briefly unreachable under load
/// must not accumulate its way to a spurious exit over a long-lived session.
const PARENT_PROBE_FAILURES_BEFORE_EXIT: u32 = 3;

/// Return once `probe` has reported failure `failures_before_exit` times in a
/// row, resetting the streak on every success.
///
/// Split from the TCP probe so the streak semantics are testable without a
/// socket or a real clock.
async fn wait_for_consecutive_probe_failures<P, F>(
    mut probe: P,
    interval: std::time::Duration,
    failures_before_exit: u32,
) where
    P: FnMut() -> F,
    F: std::future::Future<Output = bool>,
{
    let mut consecutive = 0_u32;
    loop {
        tokio::time::sleep(interval).await;
        if probe().await {
            consecutive = 0;
            continue;
        }
        consecutive += 1;
        if consecutive >= failures_before_exit {
            return;
        }
    }
}

/// Ask the parent whether this child's token is still registered.
///
/// Answers both questions that matter with one round trip: a parent that is
/// gone cannot be connected to, and a parent that no longer knows the token has
/// already made every future tool call from this child fail at its first step.
async fn probe_token_is_alive(config: &ChildConfig) -> bool {
    let request = FrameRequest {
        token: config.token.clone(),
        session_id: config.session_id.clone(),
        kind: FrameKind::TokenAlive,
        todos: Vec::new(),
        title: None,
        payload: None,
    };
    forward_to_parent(config, request, "alive").await.is_ok()
}

/// Return once this child can no longer serve anyone: its token has been
/// unregistered, or the parent is unreachable.
async fn wait_until_child_is_useless(config: ChildConfig) {
    wait_for_consecutive_probe_failures(
        || {
            let config = config.clone();
            async move { probe_token_is_alive(&config).await }
        },
        PARENT_PROBE_INTERVAL,
        PARENT_PROBE_FAILURES_BEFORE_EXIT,
    )
    .await;
}

/// Drive the rmcp server over stdio. Returns when the agent closes stdin
/// (normal disconnect), when this child has nothing left to serve, or when the
/// server fails to initialize.
async fn serve_mcp_server(config: ChildConfig) -> Result<(), String> {
    let watchdog_config = config.clone();
    let (stdin, stdout) = rmcp::transport::io::stdio();
    let service = SePlanServer { config };
    let running = serve_server(service, (stdin, stdout))
        .await
        .map_err(|e| format!("mcp server initialize failed: {e}"))?;
    tokio::select! {
        // Normal disconnect: the agent closes stdin.
        result = running.waiting() => {
            result.map_err(|e| format!("mcp server ended with error: {e}"))?;
        }
        // Fallback for the two ways stdin EOF never arrives. It only fires when
        // the AGENT closes the pipe, so an agent that outlives its own parent
        // holds us open forever; and an agent that keeps a child from a closed
        // session holds a child that can no longer answer anything, because the
        // token lookup that every call starts with now rejects it. One probe
        // settles both, and needs no cooperation from the agent.
        () = wait_until_child_is_useless(watchdog_config) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_env(port: &str, token: &str, session: &str, agent: &str) {
        std::env::set_var(ENV_PORT, port);
        std::env::set_var(ENV_TOKEN, token);
        std::env::set_var(ENV_SESSION_ID, session);
        std::env::set_var(ENV_AGENT_ID, agent);
    }

    fn clear_env() {
        std::env::remove_var(ENV_PORT);
        std::env::remove_var(ENV_TOKEN);
        std::env::remove_var(ENV_SESSION_ID);
        std::env::remove_var(ENV_AGENT_ID);
    }

    // `parse_env` reads `std::env` — these tests are not parallel-safe, so
    // serialize them with a shared lock.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parse_env_rejects_missing_port() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var(ENV_TOKEN, "tok");
        std::env::set_var(ENV_SESSION_ID, "sess");
        let err = parse_env().expect_err("missing PORT must error");
        assert!(err.contains(ENV_PORT));
        clear_env();
    }

    #[test]
    fn parse_env_rejects_missing_token() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var(ENV_PORT, "1234");
        std::env::set_var(ENV_SESSION_ID, "sess");
        let err = parse_env().expect_err("missing TOKEN must error");
        assert!(err.contains(ENV_TOKEN));
        clear_env();
    }

    #[test]
    fn parse_env_rejects_missing_session_id() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var(ENV_PORT, "1234");
        std::env::set_var(ENV_TOKEN, "tok");
        let err = parse_env().expect_err("missing SESSION_ID must error");
        assert!(err.contains(ENV_SESSION_ID));
        clear_env();
    }

    #[test]
    fn parse_env_rejects_blank_token() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        set_env("1234", "   ", "sess", "agent");
        let err = parse_env().expect_err("blank TOKEN must error");
        assert!(err.contains(ENV_TOKEN));
        clear_env();
    }

    #[test]
    fn parse_env_rejects_non_numeric_port() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        set_env("not-a-port", "tok", "sess", "agent");
        let err = parse_env().expect_err("non-numeric PORT must error");
        assert!(err.contains(ENV_PORT));
        clear_env();
    }

    #[test]
    fn parse_env_accepts_valid_config() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        set_env("4242", "tok-abc", "sess-xyz", "agent-1");
        let cfg = parse_env().expect("valid env must parse");
        assert_eq!(cfg.port, 4242);
        assert_eq!(cfg.token, "tok-abc");
        assert_eq!(cfg.session_id, "sess-xyz");
        assert_eq!(cfg.agent_id, "agent-1");
        clear_env();
    }

    #[test]
    fn parse_env_agent_id_is_optional() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        set_env("4242", "tok", "sess", "");
        std::env::remove_var(ENV_AGENT_ID);
        let cfg = parse_env().expect("AGENT_ID is optional");
        assert_eq!(cfg.agent_id, "");
        clear_env();
    }

    /// The watchdog's own tests all pass while it sits unwired — only a
    /// dead-code warning marks the gap, and warnings do not fail a build. This
    /// asserts the wiring itself.
    #[test]
    fn serve_mcp_server_races_the_watchdog_against_stdin_eof() {
        let source = include_str!("child.rs");
        let start = source
            .find("async fn serve_mcp_server(")
            .expect("serve_mcp_server");
        let end = source[start..]
            .find("\n#[cfg(test)]")
            .map(|offset| start + offset)
            .expect("serve_mcp_server end");
        let body = &source[start..end];
        assert!(
            body.contains("wait_until_child_is_useless(watchdog_config)"),
            "the usefulness fallback must be wired into the server loop"
        );
        assert!(
            body.contains("running.waiting()"),
            "the normal stdin-EOF disconnect must still end the server"
        );
        assert!(
            body.contains("tokio::select!"),
            "the two exit paths must race — awaiting either one in sequence reinstates the hang"
        );
    }

    /// End-to-end against a real parent, because the thing that matters is the
    /// contract, not the call shape: a probe that merely opened a socket would
    /// still succeed here after the session is gone, since the listener is
    /// shared by every session and stays bound.
    #[tokio::test]
    async fn probe_reads_alive_while_registered_and_dead_after_unregister() {
        let server = crate::acp::host_mcp::parent::HostPlanServer::start(vec![], None);
        let (port, token, provisional_sid) = server.register_session("agent-1");
        server.bind_session(&token, "session-real-1");
        let config = ChildConfig {
            port,
            token,
            session_id: provisional_sid,
            agent_id: "agent-1".to_string(),
        };

        assert!(
            probe_token_is_alive(&config).await,
            "a registered child must read as alive"
        );

        server.unregister_session("session-real-1");

        assert!(
            !probe_token_is_alive(&config).await,
            "once the token is dropped the child can never serve a call again"
        );
    }

    const TEST_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

    /// Drive `probe` from a fixed script; the returned counter records how many
    /// probes actually ran.
    fn scripted_probe(
        script: Vec<bool>,
    ) -> (
        impl FnMut() -> std::future::Ready<bool>,
        std::rc::Rc<std::cell::Cell<usize>>,
    ) {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0_usize));
        let counter = std::rc::Rc::clone(&calls);
        let probe = move || {
            let index = counter.get();
            counter.set(index + 1);
            // Past the end of the script the parent stays reachable, so a test
            // that expects no exit cannot pass by simply running out of script.
            std::future::ready(script.get(index).copied().unwrap_or(true))
        };
        (probe, calls)
    }

    #[tokio::test(start_paused = true)]
    async fn watchdog_returns_after_the_configured_consecutive_failures() {
        let (probe, calls) = scripted_probe(vec![false, false, false]);
        wait_for_consecutive_probe_failures(probe, TEST_INTERVAL, 3).await;
        assert_eq!(
            calls.get(),
            3,
            "must return on the third failure, not later"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn watchdog_does_not_return_while_the_parent_answers() {
        let (probe, _calls) = scripted_probe(vec![true; 8]);
        let result = tokio::time::timeout(
            TEST_INTERVAL * 20,
            wait_for_consecutive_probe_failures(probe, TEST_INTERVAL, 3),
        )
        .await;
        assert!(
            result.is_err(),
            "a reachable parent must never trip the watchdog"
        );
    }

    /// The streak resets on success. Written as the failure mode it guards:
    /// a cumulative counter would exit here on the fourth failure even though
    /// the parent answered in between.
    #[tokio::test(start_paused = true)]
    async fn watchdog_resets_the_streak_on_a_successful_probe() {
        let (probe, _calls) = scripted_probe(vec![false, false, true, false, false]);
        let result = tokio::time::timeout(
            TEST_INTERVAL * 20,
            wait_for_consecutive_probe_failures(probe, TEST_INTERVAL, 3),
        )
        .await;
        assert!(
            result.is_err(),
            "two failures, a success, then two more must not count as three in a row"
        );
    }
}
