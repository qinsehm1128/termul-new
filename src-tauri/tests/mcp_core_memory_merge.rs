//! Regression contract for merging project-scope into session-memory.
//! All indexes, transcripts, paths and credentials here are synthetic; no host
//! config, agent corpus, remote server or process-global environment is used.

use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::Arc,
};

use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult, ClientConfig},
    service::{serve_client, RoleClient, RunningService},
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
};
use se_manager_lib::{
    mcp_core::{
        default_built_ins, gateway_tools, AllowAllTools, AuthBootstrap, BuiltInRegistry,
        DenyListedTools, McpBuiltInConfig, McpCapabilityPolicy, McpControlPlaneConfig, McpCore,
        McpCoreConfig, McpHttpGateway, McpHttpGatewayConfig, ToolPermission, BUILTIN_PROJECT_SCOPE,
        BUILTIN_SESSION_MEMORY,
    },
    memory_index::{
        scope::ProjectFence,
        service::MemoryIndexService,
        store::MemoryStore,
        types::{
            FileIdentity, IndexedSession, LineageDepth, NormalizedMessage, NormalizedRole,
            SessionScope, SourcePointer, TimestampConfidence, SCHEMA_VERSION,
        },
    },
};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

const TOOLS: [&str; 6] = [
    "memory_search",
    "memory_session_list",
    "memory_session_get",
    "memory_session_messages",
    "project_boundary",
    "memory_projects",
];
const TOKEN: &str = "synthetic-memory-merge-bearer-secret";
const PRIVATE_TEXT: &str = "synthetic-other-project-private-message";

type Client = RunningService<RoleClient, ClientConfig>;

async fn connect(gateway: &McpHttpGateway, path: &str) -> Client {
    let mut headers = HashMap::new();
    headers.insert(
        HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
    );
    let config = StreamableHttpClientTransportConfig::with_uri(format!(
        "http://127.0.0.1:{}{path}",
        gateway.endpoint().port
    ))
    .custom_headers(headers);
    let transport = StreamableHttpClientTransport::from_config(config);
    serve_client(ClientConfig::default(), transport)
        .await
        .unwrap()
}

async fn http_call(client: &Client, tool: &str, arguments: Value) -> CallToolResult {
    client
        .call_tool(
            CallToolRequestParams::new(tool.to_owned())
                .with_arguments(arguments.as_object().unwrap().clone()),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn http_modes_advertise_only_canonical_memory_and_keep_hidden_aliases_callable() {
    let fixture = Fixture::new();
    let core = fixture.core(true, &default_built_ins(), Arc::new(AllowAllTools));
    let gateway = McpHttpGateway::bind(
        Arc::new(core),
        McpHttpGatewayConfig {
            bind_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            path: "/mcp".into(),
            generation: 1,
            auth: AuthBootstrap::new(1, TOKEN).unwrap(),
            request_body_limit: 64 * 1024,
            control: None,
        },
    )
    .await
    .unwrap();
    let direct = connect(&gateway, "/mcp/direct").await;
    let listed = direct.list_tools(None).await.unwrap();
    assert_eq!(
        names(listed.tools.into_iter().map(|tool| tool.name.to_string())),
        expected_names("session-memory_")
    );
    for tool in TOOLS {
        payload(
            &http_call(
                &direct,
                &format!("session-memory_{tool}"),
                fixture.arguments(tool, true),
            )
            .await,
        );
    }
    for tool in ["project_boundary", "memory_projects"] {
        let canonical = payload(
            &http_call(
                &direct,
                &format!("session-memory_{tool}"),
                fixture.arguments(tool, true),
            )
            .await,
        );
        let legacy = payload(
            &http_call(
                &direct,
                &format!("project-scope_{tool}"),
                fixture.arguments(tool, true),
            )
            .await,
        );
        assert_eq!(canonical, legacy);
    }
    let grouped = connect(&gateway, "/mcp").await;
    let advertised = grouped.list_tools(None).await.unwrap();
    assert_eq!(
        names(
            advertised
                .tools
                .into_iter()
                .map(|tool| tool.name.to_string())
        ),
        names([
            "session-memory_tool_list".into(),
            "session-memory_tool_call".into(),
        ])
    );
    for (server, expected) in [
        (BUILTIN_SESSION_MEMORY, expected_names("")),
        (
            BUILTIN_PROJECT_SCOPE,
            names(["project_boundary".into(), "memory_projects".into()]),
        ),
    ] {
        let catalog =
            payload(&http_call(&grouped, &format!("{server}_tool_list"), json!({})).await);
        assert_eq!(
            names(
                catalog["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|tool| tool["name"].as_str().unwrap().to_owned())
            ),
            expected
        );
        for tool in TOOLS {
            if server == BUILTIN_PROJECT_SCOPE
                && !matches!(tool, "project_boundary" | "memory_projects")
            {
                continue;
            }
            payload(
                &http_call(
                    &grouped,
                    &format!("{server}_tool_call"),
                    json!({
                        "toolName": tool, "arguments": fixture.arguments(tool, true),
                    }),
                )
                .await,
            );
        }
    }
    grouped.cancel().await.unwrap();
    for path in ["/mcp/entry"] {
        let client = connect(&gateway, path).await;
        let listed = client.list_tools(None).await.unwrap();
        let expected = names(
            gateway_tools::entry_tools()
                .into_iter()
                .map(|tool| tool.name.to_string()),
        );
        assert_eq!(expected.len(), 4);
        assert_eq!(
            names(listed.tools.into_iter().map(|tool| tool.name.to_string())),
            expected,
            "gateway facade at {path}"
        );
        let servers =
            payload(&http_call(&client, gateway_tools::ENTRY_LIST_SERVERS, json!({})).await);
        let servers = servers["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], BUILTIN_SESSION_MEMORY);
        let catalog = payload(
            &http_call(
                &client,
                gateway_tools::ENTRY_LIST_TOOLS,
                json!({"mcpName": BUILTIN_SESSION_MEMORY}),
            )
            .await,
        );
        let catalog = catalog["tools"].as_array().unwrap();
        assert_eq!(
            names(
                catalog
                    .iter()
                    .map(|tool| tool["name"].as_str().unwrap().to_owned())
            ),
            expected_names("")
        );
        for tool in TOOLS {
            let call = http_call(
                &client,
                gateway_tools::ENTRY_CALL_TOOL,
                json!({
                    "mcpName": BUILTIN_SESSION_MEMORY, "toolName": tool,
                    "arguments": fixture.arguments(tool, true),
                }),
            )
            .await;
            payload(&call);
        }
        for tool in ["project_boundary", "memory_projects"] {
            let call = http_call(
                &client,
                gateway_tools::ENTRY_CALL_TOOL,
                json!({
                    "mcpName": BUILTIN_PROJECT_SCOPE, "toolName": tool,
                    "arguments": fixture.arguments(tool, true),
                }),
            )
            .await;
            payload(&call);
        }
        client.cancel().await.unwrap();
    }
    direct.cancel().await.unwrap();
    gateway.shutdown().await;
}

struct Fixture {
    _temp: tempfile::TempDir,
    memory: Arc<MemoryIndexService>,
    root: PathBuf,
    other: PathBuf,
    key: String,
    other_key: String,
    session: String,
    other_session: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        // Keep private ancestors separate from the safe project display label.
        let private = temp.path().join("private-ancestor-secret");
        let root = private.join("alpha");
        let other = private.join("beta");
        let state = private.join("host-private-index");
        for path in [&root, &other, &state] {
            std::fs::create_dir_all(path).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let other = other.canonicalize().unwrap();
        let memory = Arc::new(MemoryIndexService::new(state.canonicalize().unwrap()));
        let (key, session) = seed(&memory, &root, "alpha", "mergeprobe alpha conversation");
        let (other_key, other_session) = seed(&memory, &other, "beta", PRIVATE_TEXT);
        Self {
            _temp: temp,
            memory,
            root,
            other,
            key,
            other_key,
            session,
            other_session,
        }
    }

    fn registry(&self, bound: bool) -> BuiltInRegistry {
        BuiltInRegistry::memory_backed(Arc::clone(&self.memory), bound.then(|| self.root.clone()))
    }

    fn core(
        &self,
        bound: bool,
        configs: &[McpBuiltInConfig],
        permission: Arc<dyn ToolPermission>,
    ) -> McpCore {
        let mut registry = self.registry(bound);
        registry.apply_config(configs);
        McpCore::new_with_builtins(McpCoreConfig::default(), permission, registry)
    }

    fn arguments(&self, tool: &str, bound: bool) -> Value {
        let mut args = json!({});
        if !bound && tool != "memory_projects" {
            args["project"] = json!(self.key);
        }
        match tool {
            "memory_search" => args["query"] = json!("mergeprobe"),
            "memory_session_get" | "memory_session_messages" => {
                args["sessionKey"] = json!(self.session);
            }
            _ => {}
        }
        args
    }

    fn assert_redacted(&self, text: &str) {
        for sensitive in [
            self._temp.path().to_str().unwrap(),
            self.root.to_str().unwrap(),
            self.other.to_str().unwrap(),
            self.memory.state_root().to_str().unwrap(),
            "private-ancestor-secret",
            TOKEN,
            PRIVATE_TEXT,
        ] {
            assert!(
                !text.contains(sensitive),
                "sensitive data leaked: {sensitive}"
            );
        }
    }
}

fn seed(memory: &MemoryIndexService, root: &Path, id: &str, text: &str) -> (String, String) {
    let fence = ProjectFence::single(root).unwrap();
    let location = fence.index_location(memory.state_root()).unwrap();
    location.ensure_dir().unwrap();
    let transcript = root.join(format!("{id}-synthetic-transcript.jsonl"));
    let bytes = format!("{}\n", json!({"role": "user", "text": text})).into_bytes();
    std::fs::write(&transcript, &bytes).unwrap();
    let identity = FileIdentity::read(&transcript).unwrap();
    let source = SourcePointer::for_record(&identity, transcript.to_str().unwrap(), 0, &bytes);
    let key = format!("claude-code:synthetic-{id}");
    let session = IndexedSession {
        schema_version: SCHEMA_VERSION,
        session_key: key.clone(),
        vendor: "claude-code".into(),
        vendor_session_id: format!("synthetic-{id}"),
        root_session_key: key.clone(),
        lineage_depth: LineageDepth::ROOT,
        project_key: location.namespace_key.clone(),
        scope: SessionScope::Scoped,
        cwd: Some(root.to_string_lossy().into_owned()),
        title: Some(format!("Synthetic {id}")),
        first_message_at_utc: Some("2026-09-01T00:00:00.000Z".into()),
        first_message_at_ms: Some(1_788_220_800_000),
        last_activity_at_utc: None,
        last_activity_at_ms: None,
        timestamp_confidence: TimestampConfidence::Native,
        message_count: 1,
        tool_count: 0,
        file_path: transcript.to_string_lossy().into_owned(),
        source: source.clone(),
    };
    let message = NormalizedMessage {
        schema_version: SCHEMA_VERSION,
        message_key: format!("{key}#0"),
        session_key: key.clone(),
        root_session_key: key.clone(),
        lineage_depth: LineageDepth::ROOT,
        ordinal: 0,
        role: NormalizedRole::User,
        timestamp_utc: session.first_message_at_utc.clone(),
        timestamp_ms: session.first_message_at_ms,
        timestamp_confidence: TimestampConfidence::Native,
        text: text.into(),
        tool_name: None,
        tool_call_id: None,
        source,
    };
    let mut store = MemoryStore::open(&location.database_path, &location.namespace_key).unwrap();
    store.write_project_root(root).unwrap();
    store.replace_session(&session, &[message], &[], 0).unwrap();
    (location.namespace_key, key)
}

fn args(value: Value) -> Option<Map<String, Value>> {
    Some(value.as_object().unwrap().clone())
}

fn names(items: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    items.into_iter().collect()
}

fn expected_names(prefix: &str) -> BTreeSet<String> {
    names(TOOLS.map(|tool| format!("{prefix}{tool}")))
}

fn text(result: &CallToolResult) -> &str {
    &result.content[0].as_text().unwrap().text
}

fn payload(result: &CallToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "{}", text(result));
    serde_json::from_str(text(result)).unwrap()
}

fn config(id: &str, enabled: bool, allow: Option<&[&str]>, deny: &[&str]) -> McpBuiltInConfig {
    McpBuiltInConfig {
        id: id.into(),
        enabled,
        policy: McpCapabilityPolicy {
            allow_tools: allow.map(|tools| tools.iter().map(|tool| (*tool).into()).collect()),
            deny_tools: deny.iter().map(|tool| (*tool).into()).collect(),
        },
    }
}

#[test]
fn defaults_only_add_session_memory_but_explicit_legacy_config_round_trips() {
    let defaults = default_built_ins();
    let memory_defaults = defaults
        .iter()
        .filter(|item| {
            matches!(
                item.id.as_str(),
                BUILTIN_SESSION_MEMORY | BUILTIN_PROJECT_SCOPE
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(memory_defaults.len(), 1);
    assert_eq!(memory_defaults[0].id, BUILTIN_SESSION_MEMORY);
    assert!(memory_defaults[0].enabled);
    assert!(memory_defaults[0].policy.is_empty());
    assert_eq!(McpControlPlaneConfig::empty().built_ins, defaults);
    let legacy = config(
        BUILTIN_PROJECT_SCOPE,
        false,
        Some(&["project_boundary"]),
        &["memory_projects"],
    );
    let stored = json!({"schemaVersion": 1, "revision": 7, "builtIns": [legacy], "upstreams": []});
    let parsed = McpControlPlaneConfig::from_stored_json(&stored)
        .unwrap()
        .config;
    assert_eq!(
        parsed
            .built_ins
            .iter()
            .filter(|item| matches!(
                item.id.as_str(),
                BUILTIN_SESSION_MEMORY | BUILTIN_PROJECT_SCOPE
            ))
            .count(),
        2
    );
    assert_eq!(
        parsed
            .built_ins
            .iter()
            .find(|item| item.id == BUILTIN_PROJECT_SCOPE),
        Some(&legacy)
    );
    let round_trip = McpControlPlaneConfig::from_stored_json(&parsed.to_canonical_json().unwrap())
        .unwrap()
        .config;
    assert_eq!(round_trip, parsed);
    let fresh = McpControlPlaneConfig::from_stored_json(&json!([]))
        .unwrap()
        .config;
    assert_eq!(fresh.built_ins, defaults);
}

#[tokio::test]
async fn registry_and_core_advertise_one_provider_and_six_canonical_tools() {
    let fixture = Fixture::new();
    let registry = fixture.registry(false);
    assert_eq!(
        registry.enabled_ids(),
        names([BUILTIN_SESSION_MEMORY.into()])
    );
    let listed = registry.list_tools(&AllowAllTools);
    assert_eq!(listed.len(), 6);
    assert!(listed
        .iter()
        .all(|item| item.server_id == BUILTIN_SESSION_MEMORY));
    assert_eq!(
        names(listed.into_iter().map(|item| item.tool.name.to_string())),
        expected_names("session-memory_")
    );
    let core = fixture.core(false, &default_built_ins(), Arc::new(AllowAllTools));
    let summaries = core.server_summaries().await;
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, BUILTIN_SESSION_MEMORY);
    assert!(summaries[0].built_in);
    assert_eq!(core.built_in_ids().await, [BUILTIN_SESSION_MEMORY]);
    assert_eq!(
        names(
            core.list_server_tools(BUILTIN_SESSION_MEMORY)
                .await
                .unwrap()
                .into_iter()
                .map(|tool| tool.name.to_string())
        ),
        expected_names("")
    );
    let catalogs = core.list_facade_catalogs().await.unwrap();
    assert_eq!(catalogs.len(), 1);
    assert_eq!(catalogs[0].server_id, BUILTIN_SESSION_MEMORY);
    assert_eq!(catalogs[0].tools.len(), 6);
    fixture.assert_redacted(&serde_json::to_string(&catalogs[0].tools).unwrap());
    for tool in &TOOLS[..4] {
        assert!(
            core.call_tool(
                &format!("project-scope_{tool}"),
                args(fixture.arguments(tool, false))
            )
            .await
            .is_err(),
            "legacy aliases are only defined for moved tools"
        );
    }
    for tool in TOOLS {
        let result = core
            .call_tool(
                &format!("session-memory_{tool}"),
                args(fixture.arguments(tool, false)),
            )
            .await
            .unwrap();
        let value = payload(&result);
        match tool {
            "memory_search" => assert_eq!(value["hits"].as_array().unwrap().len(), 1),
            "memory_session_list" => assert!(text(&result).contains(&fixture.session)),
            "memory_session_get" | "memory_session_messages" => {
                assert!(text(&result).contains("mergeprobe alpha conversation"))
            }
            "project_boundary" => assert_eq!(value["projectKey"], fixture.key),
            "memory_projects" => {
                assert_eq!(value.as_array().unwrap().len(), 2);
                fixture.assert_redacted(text(&result));
            }
            _ => unreachable!(),
        }
        assert!(!text(&result).contains(PRIVATE_TEXT));
    }
}

fn policy_cases(id: &str) -> Vec<Option<McpBuiltInConfig>> {
    vec![
        None,
        Some(config(id, true, None, &[])),
        Some(config(id, false, None, &[])),
        Some(config(id, true, Some(&[]), &[])),
        Some(config(id, true, Some(&["project_boundary"]), &[])),
        Some(config(id, true, Some(&["memory_projects"]), &[])),
        Some(config(id, true, Some(&TOOLS), &[])),
        Some(config(id, true, None, &["project_boundary"])),
        Some(config(id, true, None, &["memory_projects"])),
        // Deny wins even when the same tool is explicitly allowed.
        Some(config(
            id,
            true,
            Some(&TOOLS),
            &["project_boundary", "memory_projects"],
        )),
    ]
}

fn allowed(config: &Option<McpBuiltInConfig>, tool: &str) -> bool {
    config
        .as_ref()
        .is_none_or(|config| config.enabled && config.policy.allows(tool))
}

#[tokio::test]
async fn canonical_and_legacy_config_intersection_controls_listing_and_both_call_forms() {
    let fixture = Fixture::new();
    for canonical in policy_cases(BUILTIN_SESSION_MEMORY) {
        for legacy in policy_cases(BUILTIN_PROJECT_SCOPE) {
            let configs: Vec<_> = canonical.iter().chain(legacy.iter()).cloned().collect();
            let core = fixture.core(true, &configs, Arc::new(AllowAllTools));
            let listed = core.list_tools().await.unwrap();
            let listed = names(
                listed
                    .items
                    .into_iter()
                    .map(|item| item.tool.name.to_string()),
            );
            let canonical_catalog = names(
                core.list_server_tools(BUILTIN_SESSION_MEMORY)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tool| tool.name.to_string()),
            );
            let legacy_catalog = names(
                core.list_server_tools(BUILTIN_PROJECT_SCOPE)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tool| tool.name.to_string()),
            );
            for tool in TOOLS {
                let moved = matches!(tool, "project_boundary" | "memory_projects");
                let expected = allowed(&canonical, tool) && (!moved || allowed(&legacy, tool));
                assert_eq!(
                    canonical_catalog.contains(tool),
                    expected,
                    "canonical catalog: {canonical:?} {legacy:?} {tool}"
                );
                assert_eq!(
                    legacy_catalog.contains(tool),
                    expected && moved,
                    "legacy catalog: {canonical:?} {legacy:?} {tool}"
                );
                let exposed = format!("session-memory_{tool}");
                assert_eq!(
                    listed.contains(&exposed),
                    expected,
                    "canonical={canonical:?} legacy={legacy:?} tool={tool}"
                );
                let result = core
                    .call_tool(&exposed, args(fixture.arguments(tool, true)))
                    .await;
                assert_eq!(result.is_ok(), expected, "canonical call: canonical={canonical:?} legacy={legacy:?} tool={tool} result={result:?}");
                if let Ok(result) = &result {
                    assert_ne!(result.is_error, Some(true));
                }
                if !moved {
                    continue;
                }
                let alias = format!("project-scope_{tool}");
                assert!(
                    !listed.contains(&alias),
                    "compatibility aliases must stay hidden"
                );
                let result = core
                    .call_tool(&alias, args(fixture.arguments(tool, true)))
                    .await;
                assert_eq!(result.is_ok(), expected, "alias call: canonical={canonical:?} legacy={legacy:?} tool={tool} result={result:?}");
                if let Ok(result) = &result {
                    assert_ne!(result.is_error, Some(true));
                }
                for server in [BUILTIN_SESSION_MEMORY, BUILTIN_PROJECT_SCOPE] {
                    let result = core
                        .call_server_tool(
                            server,
                            tool,
                            args(fixture.arguments(tool, true)),
                            CancellationToken::new(),
                        )
                        .await;
                    assert_eq!(result.is_ok(), expected, "server call: {server}/{tool} canonical={canonical:?} legacy={legacy:?} result={result:?}");
                    let gateway = gateway_tools::call_entry(
                        &core,
                        gateway_tools::ENTRY_CALL_TOOL,
                        json!({"mcpName": server, "toolName": tool, "arguments": fixture.arguments(tool, true)}).as_object().unwrap().clone(),
                        CancellationToken::new(),
                    ).await.unwrap();
                    assert_eq!(
                        gateway.is_error != Some(true),
                        expected,
                        "gateway bypass {server}/{tool}: {gateway:?}"
                    );
                }
            }
            assert!(core
                .server_summaries()
                .await
                .iter()
                .all(|summary| summary.name != BUILTIN_PROJECT_SCOPE));
        }
    }
}

#[tokio::test]
async fn tool_permission_denials_under_either_id_cannot_be_bypassed_with_an_alias() {
    let fixture = Fixture::new();
    for denied_server in [BUILTIN_SESSION_MEMORY, BUILTIN_PROJECT_SCOPE] {
        for denied_tool in [
            "project_boundary",
            "memory_projects",
            "memory_search",
            "memory_session_list",
            "memory_session_get",
            "memory_session_messages",
        ] {
            let permission = DenyListedTools::new([(denied_server.into(), denied_tool.into())]);
            let core = fixture.core(true, &default_built_ins(), Arc::new(permission));
            let listed = names(
                core.list_tools()
                    .await
                    .unwrap()
                    .items
                    .into_iter()
                    .map(|item| item.tool.name.to_string()),
            );
            for tool in TOOLS {
                let moved = matches!(tool, "project_boundary" | "memory_projects");
                let expected =
                    !(tool == denied_tool && (moved || denied_server == BUILTIN_SESSION_MEMORY));
                let exposed = format!("session-memory_{tool}");
                assert_eq!(
                    listed.contains(&exposed),
                    expected,
                    "{denied_server}/{denied_tool} => {tool}"
                );
                let result = core
                    .call_tool(&exposed, args(fixture.arguments(tool, true)))
                    .await;
                assert_eq!(
                    result.is_ok(),
                    expected,
                    "{denied_server}/{denied_tool} => {tool}: {result:?}"
                );
                if moved {
                    let result = core
                        .call_tool(
                            &format!("project-scope_{tool}"),
                            args(fixture.arguments(tool, true)),
                        )
                        .await;
                    assert_eq!(result.is_ok(), expected, "permission bypass via alias: {denied_server}/{denied_tool} => {tool}: {result:?}");
                    let result = core
                        .call_server_tool(
                            BUILTIN_PROJECT_SCOPE,
                            tool,
                            args(fixture.arguments(tool, true)),
                            CancellationToken::new(),
                        )
                        .await;
                    assert_eq!(
                        result.is_ok(),
                        expected,
                        "permission bypass via server call"
                    );
                }
            }
        }
    }
}

#[derive(Default)]
struct RecordingPermission(std::sync::Mutex<BTreeSet<(String, String)>>);
impl ToolPermission for RecordingPermission {
    fn allows_tool(&self, server_id: &str, tool: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .insert((server_id.into(), tool.into()));
        true
    }
}

#[tokio::test]
async fn moved_tools_check_both_permission_ids_even_without_legacy_config() {
    let fixture = Fixture::new();
    let permission = Arc::new(RecordingPermission::default());
    let core = fixture.core(true, &default_built_ins(), permission.clone());
    for tool in TOOLS {
        for prefix in if matches!(tool, "project_boundary" | "memory_projects") {
            vec![BUILTIN_SESSION_MEMORY, BUILTIN_PROJECT_SCOPE]
        } else {
            vec![BUILTIN_SESSION_MEMORY]
        } {
            permission.0.lock().unwrap().clear();
            payload(
                &core
                    .call_tool(
                        &format!("{prefix}_{tool}"),
                        args(fixture.arguments(tool, true)),
                    )
                    .await
                    .unwrap(),
            );
            let mut expected = BTreeSet::from([(BUILTIN_SESSION_MEMORY.into(), tool.into())]);
            if matches!(tool, "project_boundary" | "memory_projects") {
                expected.insert((BUILTIN_PROJECT_SCOPE.into(), tool.into()));
            }
            assert_eq!(
                *permission.0.lock().unwrap(),
                expected,
                "permission identities checked for {prefix}/{tool}"
            );
        }
    }
}

#[tokio::test]
async fn bound_and_unbound_scope_isolation_and_redacted_failures_survive_aliases() {
    let fixture = Fixture::new();
    let bound = fixture.core(true, &default_built_ins(), Arc::new(AllowAllTools));
    let unbound = fixture.core(false, &default_built_ins(), Arc::new(AllowAllTools));
    for (core, bound_scope) in [(&bound, true), (&unbound, false)] {
        for prefix in [BUILTIN_SESSION_MEMORY, BUILTIN_PROJECT_SCOPE] {
            let result = core
                .call_tool(&format!("{prefix}_memory_projects"), args(json!({})))
                .await
                .unwrap();
            let value = payload(&result);
            let projects = value.as_array().unwrap();
            assert_eq!(projects.len(), if bound_scope { 1 } else { 2 });
            assert!(projects.iter().any(|project| project["key"] == fixture.key));
            if bound_scope {
                assert!(!text(&result).contains(&fixture.other_key));
            }
            fixture.assert_redacted(text(&result));
            for project in projects {
                let fields = names(project.as_object().unwrap().keys().cloned());
                assert_eq!(fields, names(["key".into(), "label".into()]));
            }
        }
    }
    for core in [&bound, &unbound] {
        for server in [BUILTIN_SESSION_MEMORY, BUILTIN_PROJECT_SCOPE] {
            for invalid in [
                json!({"project": fixture.other_key}),
                json!({"projectRoot": fixture.other}),
                json!({"secret": TOKEN}),
            ] {
                let error = core
                    .call_tool(&format!("{server}_memory_projects"), args(invalid))
                    .await
                    .unwrap_err();
                fixture.assert_redacted(&format!("{error:?} {error}"));
            }
        }
    }
    // All project-specific tools require an explicit selector when unbound.
    for tool in TOOLS.into_iter().filter(|tool| *tool != "memory_projects") {
        let missing = fixture.arguments(tool, true);
        let error = unbound
            .call_tool(&format!("session-memory_{tool}"), args(missing))
            .await
            .unwrap_err();
        fixture.assert_redacted(&format!("{error:?} {error}"));
        // Both supported selectors must respect the bound project fence.
        for selector_name in ["project", "projectRoot"] {
            let selector = if selector_name == "project" {
                fixture.other_key.clone()
            } else {
                fixture.other.to_string_lossy().into_owned()
            };
            let mut foreign = fixture.arguments(tool, true);
            foreign[selector_name] = json!(selector);
            let error = bound
                .call_tool(&format!("session-memory_{tool}"), args(foreign.clone()))
                .await
                .unwrap_err();
            fixture.assert_redacted(&format!("{error:?} {error}"));
            if tool == "project_boundary" {
                let error = bound
                    .call_tool("project-scope_project_boundary", args(foreign))
                    .await
                    .unwrap_err();
                fixture.assert_redacted(&format!("{error:?} {error}"));
            }
        }
        // Malformed args may not echo submitted paths or credentials.
        let error = unbound
            .call_tool(
                &format!("session-memory_{tool}"),
                args(json!({
                    "project": fixture.other.to_string_lossy(), "secret": TOKEN
                })),
            )
            .await
            .unwrap_err();
        fixture.assert_redacted(&format!("{error:?} {error}"));
    }
    for core in [&bound, &unbound] {
        for tool in ["memory_session_get", "memory_session_messages"] {
            let mut request = fixture.arguments(tool, false);
            request["sessionKey"] = json!(fixture.other_session);
            let result = core
                .call_tool(&format!("session-memory_{tool}"), args(request))
                .await
                .unwrap();
            let value = payload(&result);
            if tool == "memory_session_get" {
                assert!(value.is_null());
            } else {
                assert!(value["messages"].as_array().unwrap().is_empty());
            }
            fixture.assert_redacted(text(&result));
        }
    }
    for exposed in [
        "session-memory_project_boundary",
        "project-scope_project_boundary",
    ] {
        for invalid in [
            format!("{}/nonexistent-secret", fixture.other.display()),
            TOKEN.into(),
        ] {
            let error = unbound
                .call_tool(exposed, args(json!({"project": invalid})))
                .await
                .unwrap_err();
            fixture.assert_redacted(&format!("{error:?} {error}"));
        }
    }
}
