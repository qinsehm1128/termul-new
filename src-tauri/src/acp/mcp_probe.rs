//! On-demand MCP client probe.
//!
//! Opens a fresh rmcp client connection to a configured MCP server, completes
//! the `initialize` handshake, calls `tools/list`, then closes — and reports
//! the connected/disconnected status plus the tool list. The probe is
//! **on-demand only**: each invocation opens a brand-new connection and tears
//! it down immediately after `tools/list` returns (or fails). There are no
//! persistent always-on connections.
//!
//! The probe is stateless: it takes a renderer-supplied [`McpServerConfig`] and
//! does NOT touch the persisted registry. This mirrors `acp_probe_runtime`
//! (stateless Tauri command) — the renderer already holds the full config and
//! passes it through.
//!
//! ## Transport mapping
//!
//! - `stdio` → rmcp `transport-child-process` (`TokioChildProcess`). On Windows
//!   the child is spawned with `CREATE_NO_WINDOW` (`0x0800_0000`) so a GUI-launched
//!   probe does not flash a console window (mirrors the vendored ACP patch in
//!   `vendor/agent-client-protocol/src/acp_agent.rs`).
//! - `http` → rmcp `transport-streamable-http-client-reqwest`
//!   (`StreamableHttpClientTransport`).
//! - `sse` → rmcp 1.7.0 removed the standalone legacy SSE transport
//!   (CHANGELOG #562). The `client-side-sse` feature ships the SSE stream
//!   parser consumed by the streamable-http client (which still speaks
//!   `text/event-stream`). `type: 'sse'` servers are therefore probed via the
//!   modern streamable-http client; pure-legacy SSE-only servers may not probe
//!   correctly. This is a known residual risk — see the spec's Design Notes.
//!
//! Env values in stdio configs are `$VAR`/`${VAR}`-expanded before spawn
//! (unset variable → empty string, matching shell behavior). Header values are
//! NOT expanded (headers are HTTP-only; shell expansion does not apply).
//!
//! Probe outcomes are logged (connected/disconnected + server name + transport)
//! WITHOUT env/header values, tokens, or credentials.

use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;
use std::time::Duration;

use crate::mcp_core::oauth::McpOAuthConfig;
use rmcp::model::ClientConfig;
use rmcp::service::{serve_client, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Probe deadline. A hanging server (dead URL, blocking stdio) must not pin
/// the probe forever — cap the whole initialize + tools/list round-trip.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// A name=value pair (env var or HTTP header) as the renderer serializes it.
/// Mirrors `McpEnvVar` / `McpHeader` in `src/renderer/lib/acp-api.ts`.
///
/// `Debug` is implemented manually to redact `value` (env values and HTTP
/// header values frequently carry secrets). A derived `Debug` would print the
/// raw value to any tracing/log macro that formats the struct — defense-in-
/// depth so a future `?`/`%` log call cannot leak credentials.
#[derive(Clone, Deserialize)]
pub struct McpNameValuePair {
    pub name: String,
    pub value: String,
}

impl std::fmt::Debug for McpNameValuePair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpNameValuePair")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Renderer-supplied MCP server config. Stateless payload — the renderer holds
/// the full config (including `id`/`enabled`) and passes the wire subset here.
/// `type` defaults to `"stdio"` when omitted (mirrors `transportOf`).
///
/// Kept deliberately loose (all transport-specific fields optional) so the
/// probe does not couple to the vendored ACP `McpServer` schema — the probe
/// owns its own deserialization and never touches the registry store.
///
/// `Debug` is implemented manually to redact `env` and `headers` (whose `value`
/// fields carry secrets — see `McpNameValuePair`). Identifying fields the
/// boundary log already surfaces (`name`/`command`/`url`/`type`/`args`) stay
/// visible for diagnostics.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    pub name: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<McpNameValuePair>,
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Vec<McpNameValuePair>,
    #[serde(default)]
    pub oauth: Option<McpOAuthConfig>,
}

impl std::fmt::Debug for McpServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerConfig")
            .field("id", &self.id)
            .field("type", &self.r#type)
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env", &format!("<{} redacted>", self.env.len()))
            .field("url", &self.url)
            .field("headers", &format!("<{} redacted>", self.headers.len()))
            .finish()
    }
}

impl McpServerConfig {
    fn transport(&self) -> String {
        self.r#type.clone().unwrap_or_else(|| "stdio".to_string())
    }
}

/// A tool exposed by the probed server (`tools/list` output, trimmed to the
/// fields the UI surfaces).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProbeStatus {
    Connected,
    Disconnected,
}

/// Probe result. On `Disconnected`, `error` carries a short, value-free message
/// (no env/header values, tokens, or credentials).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub status: ProbeStatus,
    #[serde(default)]
    pub tools: Vec<McpToolInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ProbeResult {
    fn connected(tools: Vec<McpToolInfo>) -> Self {
        Self {
            status: ProbeStatus::Connected,
            tools,
            error: None,
        }
    }

    fn disconnected(error: impl Into<String>) -> Self {
        Self {
            status: ProbeStatus::Disconnected,
            tools: Vec::new(),
            error: Some(error.into()),
        }
    }
}

/// Expand `$VAR` and `${VAR}` references in `value` against the process
/// environment. Unset variables expand to the empty string (matching POSIX
/// shell behavior). Non-UTF8 env values are skipped.
///
/// There is no existing cross-platform expander in the tree
/// (`pty/env_refresh.rs` is Windows-only and private), so this small helper
/// owns the behavior. It is `pub(crate)` so the test module can exercise it.
pub(crate) fn expand_env(value: &str) -> String {
    expand_env_with(value, |name| std::env::var(name).ok())
}

/// Testable core: `lookup` provides the variable map (the real expander uses
/// `std::env::var`; tests inject a fake map). Unset → `None` → empty string.
fn expand_env_with(value: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while !rest.is_empty() {
        match rest.find('$') {
            None => {
                out.push_str(rest);
                break;
            }
            Some(dollar) => {
                out.push_str(&rest[..dollar]);
                let after = &rest[dollar + 1..];
                // `${VAR}` braced form.
                if let Some(stripped) = after.strip_prefix('{') {
                    match stripped.find('}') {
                        Some(end) => {
                            let name = &stripped[..end];
                            out.push_str(&lookup(name).unwrap_or_default());
                            rest = &stripped[end + 1..];
                        }
                        None => {
                            // Unterminated `${` — treat literally (no expansion).
                            out.push('$');
                            out.push_str(after);
                            break;
                        }
                    }
                } else {
                    // `$VAR` bare form: [A-Za-z_][A-Za-z0-9_]*.
                    let end = after
                        .char_indices()
                        .take_while(|(i, c)| {
                            let first = *i == 0;
                            c.is_ascii_alphabetic() || (c == &'_') || (!first && c.is_ascii_digit())
                        })
                        .last()
                        .map(|(i, c)| i + c.len_utf8())
                        .unwrap_or(0);
                    if end == 0 {
                        // Lone `$` not followed by a valid name — emit literally.
                        out.push('$');
                        rest = after;
                    } else {
                        let name = &after[..end];
                        out.push_str(&lookup(name).unwrap_or_default());
                        rest = &after[end..];
                    }
                }
            }
        }
    }
    out
}

/// One-shot probe: open a fresh rmcp client connection, `initialize`, list
/// tools, close. Never panics, never logs secrets — only the server name +
/// transport + outcome.
fn classify_probe_error(error: impl std::fmt::Display) -> String {
    let rendered = error.to_string();
    let lower = rendered.to_ascii_lowercase();
    if lower.contains("auth required")
        || lower.contains("401 unauthorized")
        || (lower.contains("authorization")
            && (lower.contains("cannot authenticate")
                || lower.contains("authentication")
                || lower.contains("auth")))
    {
        return "remote MCP server requires a valid Authorization header; check headers or bearerToken in the MCP JSON".into();
    }
    if lower.contains("could not parse json response as jsonrpcmessage")
        || (lower.contains("unexpected server response") && lower.contains("json"))
    {
        if lower.contains("\"success\"") || lower.contains("\"code\"") {
            return "remote endpoint returned a non-MCP JSON error; check Authorization and confirm the URL is the MCP endpoint".into();
        }
        return "remote endpoint returned invalid MCP JSON-RPC; confirm the URL and transport type"
            .into();
    }
    if lower.contains("unexpected content type") {
        return "remote endpoint returned a non-MCP content type; confirm the URL is a Streamable HTTP MCP endpoint".into();
    }
    if lower.contains("missing session id") || lower.contains("session id in response") {
        return "remote MCP endpoint requires a session response that it did not provide".into();
    }
    rendered
        .replace("Authorization: Bearer ", "Authorization: Bearer <redacted>")
        .replace("authorization: Bearer ", "authorization: Bearer <redacted>")
}

fn classify_oauth_probe_error(error: impl std::fmt::Display) -> String {
    let rendered = classify_probe_error(error);
    if rendered.contains("requires a valid Authorization header") {
        return "OAuth authorization is required or the stored grant was rejected; authorize the MCP server again"
            .into();
    }
    rendered
}

#[cfg(test)]
async fn probe(server: McpServerConfig) -> ProbeResult {
    probe_for_project(server, None).await
}

/// Probe one MCP server using credentials owned by `project_root`.
///
/// `None` keeps static-header probes working and refuses OAuth credential
/// lookup rather than falling back to a project-shared keyring account.
pub async fn probe_for_project(
    server: McpServerConfig,
    project_root: Option<&std::path::Path>,
) -> ProbeResult {
    let transport = server.transport();
    let name = server.name.clone();
    let result = tokio::time::timeout(PROBE_TIMEOUT, probe_inner(&server, project_root)).await;
    let outcome = match result {
        Ok(inner) => inner,
        Err(_elapsed) => ProbeResult::disconnected(format!(
            "probe timed out after {}s",
            PROBE_TIMEOUT.as_secs()
        )),
    };
    // Boundary log: outcome + server name + transport. NO env/header values,
    // tokens, or credentials.
    match outcome.status {
        ProbeStatus::Connected => tracing::info!(
            server = %name,
            transport = %transport,
            tools = outcome.tools.len(),
            "MCP probe connected"
        ),
        ProbeStatus::Disconnected => tracing::warn!(
            server = %name,
            transport = %transport,
            "MCP probe disconnected"
        ),
    }
    outcome
}

async fn probe_inner(
    server: &McpServerConfig,
    project_root: Option<&std::path::Path>,
) -> ProbeResult {
    let transport = server.transport();
    match transport.as_str() {
        "stdio" => probe_stdio(server).await,
        "sse"
            if server.oauth.as_ref().is_some_and(|oauth| {
                oauth.auth_mode == crate::mcp_core::oauth::McpAuthMode::OAuth
            }) =>
        {
            ProbeResult::disconnected(
                "legacy SSE upstreams are not supported for OAuth; use type=http",
            )
        }
        "http" | "sse" => probe_http(server, &transport, project_root).await,
        other => ProbeResult::disconnected(format!("unsupported transport '{other}'")),
    }
}

/// Resolve a stdio command for direct spawning (Windows shim handling).
///
/// On Windows, npm/PowerShell CLIs install as `.cmd`/`.bat` batch shims, which
/// `CreateProcessW` cannot launch directly (os error 193 / "spawn failed").
/// Reuse the PTY launcher's shim-aware resolver (ADR-004.2): it rewrites e.g.
/// `npx.cmd` to `node.exe <script>`, prepending the script ahead of the user
/// args. A resolution failure falls back to the legacy PATH/PATHEXT lookup so
/// any real spawn error stays observable. On non-Windows the bare command name
/// is returned unchanged (PATH resolution is left to the spawner).
fn resolve_stdio_command(command: &str) -> (String, Vec<String>) {
    match crate::pty::manager::resolve_spawn_program(command) {
        Ok(resolved) => (resolved.program, resolved.prepend_args),
        Err(_) => (
            crate::trackers::git_tracker::resolve_executable(command),
            Vec::new(),
        ),
    }
}

#[cfg(not(target_os = "windows"))]
fn resolve_stdio_command_in_path(command: &str, path: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;

    if command.contains('/') {
        return Some(command.to_string());
    }
    for directory in path.split(':').filter(|segment| !segment.is_empty()) {
        let candidate = std::path::Path::new(directory).join(command);
        if let Ok(metadata) = std::fs::metadata(&candidate) {
            if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

async fn probe_stdio(server: &McpServerConfig) -> ProbeResult {
    let command = match server.command.as_deref() {
        Some(c) if !c.trim().is_empty() => c,
        _ => return ProbeResult::disconnected("stdio command is required"),
    };
    let (mut program, prepend_args) = resolve_stdio_command(command);

    // A GUI-launched desktop process often inherits a minimal PATH from
    // Finder/Dock rather than the user's interactive shell. Keep MCP stdio
    // probes consistent with ACP and PTY launches by refreshing and merging
    // the login-shell PATH before spawning package-manager commands such as
    // `npx`/`uvx`. Explicit MCP env values remain in the map and win for their
    // own keys after expansion.
    let mut env_map = HashMap::new();
    for pair in &server.env {
        // Expand `$VAR`/`${VAR}` before spawn; unset → empty string.
        env_map.insert(pair.name.clone(), expand_env(&pair.value));
    }
    crate::pty::env_refresh::apply_fresh_path(&mut env_map);
    #[cfg(not(target_os = "windows"))]
    if let Some(path) = env_map.get("PATH") {
        if let Some(resolved) = resolve_stdio_command_in_path(&program, path) {
            program = resolved;
        }
    }

    let mut cmd = Command::new(&program);
    cmd.args(&prepend_args);
    cmd.args(&server.args);
    for (name, value) in env_map {
        cmd.env(name, value);
    }
    // Windows: suppress the console window a GUI-launched probe would flash.
    // CREATE_NO_WINDOW = 0x0800_0000 (mirrors the vendored ACP patch). tokio's
    // `Command` exposes `creation_flags` natively on Windows.
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(0x0800_0000);
    }
    // Kill the child if the probe future is dropped mid-flight (notably when
    // `tokio::time::timeout` fires above — dropping `probe_inner` drops the
    // `TokioChildProcess` handle; without `kill_on_drop` the child is left
    // running as an orphan). HTTP/SSE transports have no child process and are
    // unaffected. tokio's default is to leave the child running on drop.
    cmd.kill_on_drop(true);

    // Builder lets us null stderr so a chatty child does not pollute our logs.
    // stdin/stdout stay piped (the rmcp transport owns them).
    let spawn = TokioChildProcess::builder(cmd)
        .stderr(Stdio::null())
        .spawn();
    let transport = match spawn {
        Ok((proc, _stderr)) => proc,
        Err(error) => return ProbeResult::disconnected(format!("spawn failed: {error}")),
    };
    let running = match serve_client(ClientConfig::default(), transport).await {
        Ok(service) => service,
        Err(error) => {
            return ProbeResult::disconnected(format!(
                "initialize failed: {}",
                classify_probe_error(error)
            ))
        }
    };
    drive_running(running).await
}

fn is_transport_managed_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("accept")
}

async fn probe_http(
    server: &McpServerConfig,
    transport: &str,
    project_root: Option<&std::path::Path>,
) -> ProbeResult {
    let url = match server.url.as_deref() {
        Some(u) if !u.trim().is_empty() => u,
        _ => return ProbeResult::disconnected(format!("{transport} URL is required")),
    };
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    // Several hosted MCP services (including providers compatible with the
    // router client) do not return a Mcp-Session-Id and operate statelessly.
    // The probe is a one-shot initialize + tools/list connection, so requiring
    // a session here rejects otherwise valid HTTP MCP endpoints.
    config.allow_stateless = true;
    if let (Some(oauth), Some(server_id)) = (
        server
            .oauth
            .as_ref()
            .filter(|value| value.auth_mode == crate::mcp_core::oauth::McpAuthMode::OAuth),
        server.id.as_deref(),
    ) {
        let headers = server
            .headers
            .iter()
            .filter(|pair| !is_transport_managed_header(&pair.name))
            .map(|pair| (pair.name.clone(), pair.value.clone()))
            .collect::<BTreeMap<_, _>>();
        let Some(project_root) = project_root else {
            return ProbeResult::disconnected("OAuth credentials require an active project scope");
        };
        let client = match crate::mcp_core::oauth::oauth_transport(
            url,
            project_root,
            server_id,
            oauth,
            &headers,
        )
        .await
        {
            Ok(client) => client,
            Err(error) => return ProbeResult::disconnected(error),
        };
        let running = match serve_client(ClientConfig::default(), client).await {
            Ok(service) => service,
            Err(error) => {
                return ProbeResult::disconnected(format!(
                    "initialize failed: {}",
                    classify_oauth_probe_error(error)
                ))
            }
        };
        return drive_running(running).await;
    }
    if !server.headers.is_empty() {
        let mut headers = HashMap::new();
        for pair in &server.headers {
            // rmcp owns Accept negotiation. Keep imported values persisted, but
            // omit this transport-managed header from the wire request to avoid
            // ReservedHeaderConflict("accept").
            if is_transport_managed_header(&pair.name) {
                tracing::debug!(
                    server = %server.name,
                    transport,
                    header = %pair.name,
                    "skipping transport-managed MCP header"
                );
                continue;
            }
            // reqwest re-exports `http::HeaderName`/`HeaderValue` (the project
            // already depends on reqwest); avoids a direct `http` dep here.
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(pair.name.as_bytes()),
                reqwest::header::HeaderValue::from_str(&pair.value),
            ) {
                headers.insert(name, value);
            } else {
                // Skip a malformed header — do NOT surface the value.
                tracing::warn!(
                    server = %server.name,
                    transport,
                    "skipping malformed MCP header (value redacted)"
                );
            }
        }
        config = config.custom_headers(headers);
    }
    let client = StreamableHttpClientTransport::from_config(config);
    let running = match serve_client(ClientConfig::default(), client).await {
        Ok(service) => service,
        Err(error) => {
            return ProbeResult::disconnected(format!(
                "initialize failed: {}",
                classify_probe_error(error)
            ))
        }
    };
    drive_running(running).await
}

/// Drive an initialized rmcp client service: list all tools (paginated), then
/// cancel (tear down the connection — the probe is one-shot). Maps the rmcp
/// `Tool` model to the trimmed `McpToolInfo` the UI surfaces.
async fn drive_running(
    running: RunningService<rmcp::service::RoleClient, ClientConfig>,
) -> ProbeResult {
    let tools = match running.list_all_tools().await {
        Ok(tools) => tools,
        Err(error) => {
            let _ = running.cancel().await;
            return ProbeResult::disconnected(format!(
                "tools/list failed: {}",
                classify_probe_error(error)
            ));
        }
    };
    let mapped = tools
        .into_iter()
        .map(|tool| McpToolInfo {
            name: tool.name.to_string(),
            description: tool.description.map(|cow| cow.to_string()),
        })
        .collect();
    // Tear down the connection (one-shot probe — no persistent hold).
    let _ = running.cancel().await;
    ProbeResult::connected(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_from(map: &HashMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn expands_bare_and_braced_references() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        env.insert("PATH".to_string(), "/bin".to_string());
        let expand = |v: &str| expand_env_with(v, lookup_from(&env));

        assert_eq!(expand("$FOO"), "bar");
        assert_eq!(expand("${FOO}"), "bar");
        assert_eq!(expand("prefix:$FOO:suffix"), "prefix:bar:suffix");
        // Literal `/` between `$FOO` (bar) and `$PATH` (/bin) yields a double
        // slash — matching POSIX shell behavior (`echo "$FOO/$PATH"` → bar//bin).
        assert_eq!(expand("$FOO/$PATH"), "bar//bin");
        assert_eq!(expand("${FOO}-${PATH}"), "bar-/bin");
        // Unset → empty string (POSIX shell behavior).
        assert_eq!(expand("$NOPE"), "");
        assert_eq!(expand("${NOPE}"), "");
        assert_eq!(expand("x=$NOPE:y"), "x=:y");
        // Lone $ and non-variable characters emitted literally.
        assert_eq!(expand("cost is $5"), "cost is $5");
        assert_eq!(expand("100%"), "100%");
        // Adjacent references and trailing text.
        assert_eq!(expand("$FOO$PATH"), "bar/bin");
        assert_eq!(expand("$FOO tail"), "bar tail");
        // Unterminated `${` is left literally (no expansion, no panic).
        assert_eq!(expand("a${UNCLOSED"), "a${UNCLOSED");
        // Empty input.
        assert_eq!(expand(""), "");
    }

    #[test]
    fn classifies_remote_auth_and_non_mcp_json_errors_without_credentials() {
        assert_eq!(
            classify_probe_error(
                "unexpected server response: could not parse JSON response as JsonRpcMessage: {\"code\":1001,\"msg\":\"Authorization required\"}"
            ),
            "remote MCP server requires a valid Authorization header; check headers or bearerToken in the MCP JSON"
        );
        assert_eq!(
            classify_probe_error(
                "unexpected server response: could not parse JSON response as JsonRpcMessage: {\"success\":false,\"message\":\"bad request\"}"
            ),
            "remote endpoint returned a non-MCP JSON error; check Authorization and confirm the URL is the MCP endpoint"
        );
        assert_eq!(
            classify_probe_error("Auth required, when send initialize request"),
            "remote MCP server requires a valid Authorization header; check headers or bearerToken in the MCP JSON"
        );
        let redacted =
            classify_probe_error("request failed Authorization: Bearer super-secret-token");
        assert!(!redacted.contains("super-secret-token"));
    }

    #[test]
    fn rejects_unsupported_transport() {
        // The config is loose enough to accept any `type`; the probe rejects
        // unknown transports with a disconnected result (no panic).
        let config = McpServerConfig {
            r#type: Some("ftp".to_string()),
            name: "bad".to_string(),
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            url: None,
            headers: Vec::new(),
            id: None,
            oauth: None,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(probe(config));
        assert_eq!(result.status, ProbeStatus::Disconnected);
        assert!(result.error.unwrap().contains("unsupported transport"));
    }

    fn oauth_probe_server(transport: &str) -> McpServerConfig {
        McpServerConfig {
            id: Some("remote".to_string()),
            r#type: Some(transport.to_string()),
            name: "Remote".to_string(),
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            url: Some("https://mcp.example.test/mcp".to_string()),
            headers: Vec::new(),
            oauth: Some(McpOAuthConfig {
                auth_mode: crate::mcp_core::oauth::McpAuthMode::OAuth,
                ..McpOAuthConfig::default()
            }),
        }
    }

    #[tokio::test]
    async fn oauth_probe_requires_http_and_project_scope_before_credential_lookup() {
        let sse = probe_for_project(
            oauth_probe_server("sse"),
            Some(std::path::Path::new("/tmp")),
        )
        .await;
        assert_eq!(sse.status, ProbeStatus::Disconnected);
        let sse_error = sse.error.expect("sse oauth explains the transport");
        assert!(sse_error.contains("SSE"), "{sse_error}");
        assert!(!sse_error.contains("access_token"));

        let missing_scope = probe_for_project(oauth_probe_server("http"), None).await;
        assert_eq!(missing_scope.status, ProbeStatus::Disconnected);
        let scope_error = missing_scope
            .error
            .expect("http oauth without a project is refused");
        assert!(
            scope_error.contains("active project scope"),
            "{scope_error}"
        );
        assert!(!scope_error.contains("access_token"));
    }

    #[test]
    fn oauth_probe_auth_failure_does_not_ask_for_a_static_bearer() {
        let message = classify_oauth_probe_error("Auth required, when send initialize request");
        assert!(message.contains("authorize the MCP server again"));
        assert!(!message.contains("bearerToken"));
    }

    #[tokio::test]
    async fn unreachable_stdio_command_returns_disconnected() {
        // A command path that cannot exist → spawn fails → disconnected. The
        // error must NOT echo env values (none here, but the contract holds).
        let config = McpServerConfig {
            r#type: Some("stdio".to_string()),
            name: "ghost".to_string(),
            command: Some("this-binary-does-not-exist-12345".to_string()),
            args: Vec::new(),
            env: vec![McpNameValuePair {
                name: "SECRET".to_string(),
                value: "$DO_NOT_LEAK".to_string(),
            }],
            url: None,
            headers: Vec::new(),
            id: None,
            oauth: None,
        };
        let result = probe(config).await;
        assert_eq!(result.status, ProbeStatus::Disconnected);
        let error = result.error.expect("disconnected carries an error");
        assert!(
            !error.contains("DO_NOT_LEAK"),
            "error must not leak env value references: {error}"
        );
        assert!(error.contains("spawn failed"));
        assert!(result.tools.is_empty());
    }

    #[tokio::test]
    async fn missing_stdio_command_returns_disconnected() {
        let config = McpServerConfig {
            r#type: Some("stdio".to_string()),
            name: "empty".to_string(),
            command: Some("   ".to_string()),
            args: Vec::new(),
            env: Vec::new(),
            url: None,
            headers: Vec::new(),
            id: None,
            oauth: None,
        };
        let result = probe(config).await;
        assert_eq!(result.status, ProbeStatus::Disconnected);
        assert!(result.error.unwrap().contains("command is required"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolve_stdio_command_rewrites_windows_cmd_shim() {
        // Simulate an npm-installed launcher (e.g. `npx`) that exists only as a
        // `.cmd` shim — `CreateProcessW` cannot launch batch files directly, so
        // the resolver must rewrite it to the directly-executable interpreter
        // with the script prepended ahead of the user args.
        // Unique per-process dir so parallel `cargo test` invocations cannot
        // delete/overwrite each other's fixtures.
        let dir = std::env::temp_dir().join(format!(
            "se-manager-test-mcp-cmd-shim-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("node.exe"), b"MZ").unwrap();
        std::fs::create_dir_all(dir.join("node_modules\\npx\\bin")).unwrap();
        std::fs::write(dir.join("node_modules\\npx\\bin\\npx"), b"").unwrap();

        let shim_path = dir.join("npx.cmd");
        let shim_content = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\n\
            endLocal & goto #_undefined_# 2>NUL || \"%_prog%\" \"%dp0%\\node_modules\\npx\\bin\\npx\" %*\r\n";
        std::fs::write(&shim_path, shim_content).unwrap();

        let (program, prepend_args) = resolve_stdio_command(&shim_path.to_string_lossy());
        assert!(
            program.ends_with("node.exe"),
            "expected node.exe, got: {program}"
        );
        assert_eq!(prepend_args.len(), 1);
        assert!(
            prepend_args[0].contains("node_modules\\npx\\bin\\npx"),
            "expected npx script first, got: {:?}",
            prepend_args
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_only_transport_managed_headers() {
        assert!(is_transport_managed_header("Accept"));
        assert!(is_transport_managed_header("accept"));
        assert!(!is_transport_managed_header("Authorization"));
        assert!(!is_transport_managed_header("X-Workspace"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn resolves_stdio_command_from_refreshed_path() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("npx");
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let path = directory.path().to_string_lossy().into_owned();
        assert_eq!(
            resolve_stdio_command_in_path("npx", &path),
            Some(executable.to_string_lossy().into_owned())
        );
        assert_eq!(resolve_stdio_command_in_path("missing", &path), None);
    }

    #[tokio::test]
    async fn http_probe_sends_authorization_to_stateless_endpoint() {
        use axum::{
            extract::State,
            http::{header, HeaderMap, StatusCode},
            response::{IntoResponse, Response},
            routing::post,
            Json, Router,
        };
        use serde_json::{json, Value};
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        use tokio::net::TcpListener;

        async fn handle(
            State(authorized): State<Arc<AtomicBool>>,
            headers: HeaderMap,
            Json(request): Json<Value>,
        ) -> Response {
            if headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                == Some("Bearer test-token")
            {
                authorized.store(true, Ordering::SeqCst);
            }
            let method = request
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if method.starts_with("notifications/") {
                return (StatusCode::OK, "").into_response();
            }
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let result = if method == "initialize" {
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "stateless-fixture", "version": "1"}
                })
            } else {
                json!({"tools": []})
            };
            (
                [(header::CONTENT_TYPE, "application/json")],
                Json(json!({"jsonrpc": "2.0", "id": id, "result": result})),
            )
                .into_response()
        }

        let authorized = Arc::new(AtomicBool::new(false));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/mcp", post(handle))
            .with_state(Arc::clone(&authorized));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let result = probe(McpServerConfig {
            r#type: Some("http".to_string()),
            name: "stateless-fixture".to_string(),
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            url: Some(url),
            headers: vec![McpNameValuePair {
                name: "Authorization".to_string(),
                value: "Bearer test-token".to_string(),
            }],
            id: None,
            oauth: None,
        })
        .await;

        assert_eq!(result.status, ProbeStatus::Connected);
        assert!(result.tools.is_empty());
        assert!(authorized.load(Ordering::SeqCst));
        server.abort();
    }

    #[tokio::test]
    async fn unreachable_http_url_returns_disconnected() {
        // A port nothing listens on → connect failure → disconnected. The error
        // must not leak header values.
        let config = McpServerConfig {
            r#type: Some("http".to_string()),
            name: "dead".to_string(),
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            url: Some("http://127.0.0.1:1/mcp".to_string()),
            headers: vec![McpNameValuePair {
                name: "Authorization".to_string(),
                value: "Bearer super-secret".to_string(),
            }],
            id: None,
            oauth: None,
        };
        let result = probe(config).await;
        assert_eq!(result.status, ProbeStatus::Disconnected);
        let error = result.error.expect("disconnected carries an error");
        assert!(
            !error.contains("super-secret"),
            "error must not leak header values: {error}"
        );
        assert!(result.tools.is_empty());
    }
}
