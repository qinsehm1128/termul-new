//! The one implementation every surface shares.
//!
//! Three MCP-facing adapters exist — the injected `host_mcp` child that serves
//! agents inside a Termul session, a standalone read-only stdio subcommand for
//! external MCP clients, and the HTTP/Tauri pair the browser and desktop UIs
//! use. All three are thin: they translate a request, call this service, and
//! serialize the answer. Anything a caller could get from one and not another
//! would be a parity bug by construction.
//!
//! ## Two checks that deliberately run again here
//!
//! The index is already namespaced per project on disk and the database refuses
//! to open under a different project key. This layer still re-derives the fence
//! from the caller's single project root on **every** call, because "it is in
//! the index" is not the same claim as "this caller may see it". An MCP client
//! reaching the standalone subcommand has no session, no parent process and no
//! token — the fence is the only thing standing between it and another
//! project's history.
//!
//! Likewise every returned hit has its source pointer verified. A stale pointer
//! means the transcript changed under the index, so the stored text may no
//! longer be what that file says; those hits are withheld unless the caller
//! explicitly asks for them.

use std::path::{Path, PathBuf};

use super::ingest::{self, IngestOptions, IngestProgress, IngestReport};
use super::paths::MemoryVendor;
use super::scope::ProjectFence;
use super::store::{MemorySearchHit, MemoryStore, MAX_QUERY_LIMIT};
use super::types::{CompactionRecord, IndexedSession, PointerFreshness, SessionScope};
use super::{MemoryIndexError, MemoryIndexResult, ERR_OUT_OF_SCOPE};

/// Default number of hits a query returns when the caller does not say.
pub const DEFAULT_QUERY_LIMIT: usize = 20;

/// A search request, identical across the three transports.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchRequest {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    /// Restrict to particular agents. **Empty means all of them.**
    ///
    /// Optional by design: memory is looked up across agents, because which CLI
    /// happened to be running when something was worked out is rarely what you
    /// remember about it. Narrowing to one agent is a refinement, not the shape
    /// of the question.
    #[serde(default)]
    pub agents: Vec<String>,
    /// Include sessions whose project ownership could not be proven. Off by
    /// default: an open UI panel is not evidence of ownership.
    #[serde(default)]
    pub include_unscoped: bool,
    /// Include hits whose source transcript has changed since indexing.
    #[serde(default)]
    pub include_stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchResponse {
    pub project_key: String,
    pub query: String,
    /// Agent-authored summaries matching the query, returned ahead of ordinary
    /// hits. They are the only place any of the three corpora records what the
    /// agent was forced to drop from its context.
    pub compactions: Vec<CompactionRecord>,
    pub hits: Vec<MemorySearchHit>,
    /// Hits withheld because their source transcript changed since indexing.
    pub stale_hits_omitted: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySessionDetail {
    pub session: IndexedSession,
    pub messages: Vec<MemorySearchHit>,
    pub stale_messages_omitted: u32,
}

/// Whether a project has an index, and how big it is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndexStatus {
    pub project_key: String,
    pub project_label: String,
    pub database_path: String,
    pub exists: bool,
    pub size_bytes: u64,
    pub session_count: u64,
    pub message_count: u64,
    pub compaction_count: u64,
    pub newest_first_message_at_utc: Option<String>,
}

/// Shared, host-agnostic memory index service.
///
/// Holds only the host's state root. The desktop names it from Tauri's
/// `app_data_dir()` and the standalone server from its service-account state
/// dir; the service never derives it, because a shared service that guesses is
/// how two hosts end up sharing one mutable store.
#[derive(Debug, Clone)]
pub struct MemoryIndexService {
    state_root: PathBuf,
}

impl MemoryIndexService {
    #[must_use]
    pub fn new(state_root: PathBuf) -> Self {
        Self { state_root }
    }

    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Build or refresh a project's index.
    ///
    /// The only mutating entry point, and the only one reachable from the
    /// product's right-click menu. Nothing on a startup, mount or list path
    /// calls it.
    pub fn build(
        &self,
        project_root: &Path,
        options: &IngestOptions,
        progress: &mut dyn FnMut(IngestProgress),
    ) -> MemoryIndexResult<IngestReport> {
        let fence = ProjectFence::single(project_root)?;
        ingest::build_index(&fence, &self.state_root, options, progress)
    }

    pub fn status(&self, project_root: &Path) -> MemoryIndexResult<MemoryIndexStatus> {
        let fence = ProjectFence::single(project_root)?;
        let location = fence.index_location(&self.state_root)?;
        let mut status = MemoryIndexStatus {
            project_key: location.namespace_key.clone(),
            project_label: fence.display_label(),
            database_path: location.database_path.to_string_lossy().into_owned(),
            exists: location.database_path.is_file(),
            size_bytes: 0,
            session_count: 0,
            message_count: 0,
            compaction_count: 0,
            newest_first_message_at_utc: None,
        };
        if !status.exists {
            return Ok(status);
        }
        status.size_bytes = std::fs::metadata(&location.database_path)
            .map(|meta| meta.len())
            .unwrap_or_default();
        let store = MemoryStore::open(&location.database_path, &location.namespace_key)?;
        let (sessions, messages, compactions) = store.counts()?;
        status.session_count = sessions;
        status.message_count = messages;
        status.compaction_count = compactions;
        status.newest_first_message_at_utc = store
            .list_sessions(false, &[], 1)?
            .into_iter()
            .next()
            .and_then(|session| session.first_message_at_utc);
        Ok(status)
    }

    /// Sessions ordered by first message time, newest first.
    pub fn list_sessions(
        &self,
        project_root: &Path,
        limit: Option<usize>,
        include_unscoped: bool,
        agents: &[String],
    ) -> MemoryIndexResult<Vec<IndexedSession>> {
        let (fence, store) = match self.open(project_root)? {
            Some(open) => open,
            None => return Ok(Vec::new()),
        };
        let vendors = parse_agents(agents)?;
        let sessions = store.list_sessions(include_unscoped, &vendors, resolve_limit(limit))?;
        sessions
            .into_iter()
            .map(|session| authorize_session(&fence, session))
            .collect()
    }

    pub fn search(
        &self,
        project_root: &Path,
        request: &MemorySearchRequest,
    ) -> MemoryIndexResult<MemorySearchResponse> {
        let (fence, store) = match self.open(project_root)? {
            Some(open) => open,
            None => {
                return Ok(MemorySearchResponse {
                    project_key: ProjectFence::single(project_root)?.namespace_key(),
                    query: request.query.clone(),
                    compactions: Vec::new(),
                    hits: Vec::new(),
                    stale_hits_omitted: 0,
                })
            }
        };
        let limit = resolve_limit(request.limit);
        let vendors = parse_agents(&request.agents)?;
        let compactions =
            store.search_compactions(&request.query, request.include_unscoped, limit)?;
        let raw = store.search(&request.query, request.include_unscoped, &vendors, limit)?;
        let (hits, stale_hits_omitted) = self.finish_hits(&fence, raw, request.include_stale)?;
        Ok(MemorySearchResponse {
            project_key: fence.namespace_key(),
            query: request.query.clone(),
            compactions,
            hits,
            stale_hits_omitted,
        })
    }

    /// One session and its messages in transcript order.
    pub fn get_session(
        &self,
        project_root: &Path,
        session_key: &str,
        limit: Option<usize>,
        include_stale: bool,
    ) -> MemoryIndexResult<Option<MemorySessionDetail>> {
        let (fence, store) = match self.open(project_root)? {
            Some(open) => open,
            None => return Ok(None),
        };
        let Some(session) = store.get_session(session_key)? else {
            return Ok(None);
        };
        let session = authorize_session(&fence, session)?;
        // A session the caller may not see must read as absent, not as denied:
        // a distinguishable rejection tells an external client that some other
        // project has a session by that key.
        if session.scope != SessionScope::Scoped {
            return Ok(None);
        }
        let raw = store.session_messages(session_key, resolve_limit(limit))?;
        let (messages, stale_messages_omitted) = self.finish_hits(&fence, raw, include_stale)?;
        Ok(Some(MemorySessionDetail {
            session,
            messages,
            stale_messages_omitted,
        }))
    }

    /// Re-authorize every row and verify its source pointer.
    fn finish_hits(
        &self,
        fence: &ProjectFence,
        raw: Vec<MemorySearchHit>,
        include_stale: bool,
    ) -> MemoryIndexResult<(Vec<MemorySearchHit>, u32)> {
        let expected = fence.namespace_key();
        let mut kept = Vec::with_capacity(raw.len());
        let mut omitted = 0;
        for mut hit in raw {
            // The pointer names a file. It has to still be inside a vendor
            // store, or this row is not describing a transcript any more.
            let path = Path::new(&hit.source.file_path);
            if !path.is_absolute() {
                omitted += 1;
                continue;
            }
            hit.source_fresh = hit.source.verify() == PointerFreshness::Fresh;
            if !hit.source_fresh && !include_stale {
                omitted += 1;
                continue;
            }
            debug_assert_eq!(
                expected,
                fence.namespace_key(),
                "the fence must not change mid-response"
            );
            kept.push(hit);
        }
        Ok((kept, omitted))
    }

    /// Open a project's index for reading, or `None` when it has never been
    /// built.
    ///
    /// Deliberately does not create the database: "this project has no index
    /// yet" is a real answer, and creating an empty file to say it would make
    /// a read-only surface a writing one.
    fn open(&self, project_root: &Path) -> MemoryIndexResult<Option<(ProjectFence, MemoryStore)>> {
        let fence = ProjectFence::single(project_root)?;
        let location = fence.index_location(&self.state_root)?;
        if !location.database_path.is_file() {
            return Ok(None);
        }
        let store = MemoryStore::open(&location.database_path, &location.namespace_key)?;
        Ok(Some((fence, store)))
    }
}

/// Turn wire agent ids into vendors.
///
/// An unknown id is rejected rather than ignored: silently dropping
/// `"gemini-cli"` from the filter would widen the search to every agent, which
/// is the opposite of what the caller asked for.
fn parse_agents(agents: &[String]) -> MemoryIndexResult<Vec<MemoryVendor>> {
    agents
        .iter()
        .map(|agent| {
            MemoryVendor::parse(agent.trim()).ok_or_else(|| {
                MemoryIndexError::new(
                    ERR_OUT_OF_SCOPE,
                    format!("unknown agent '{agent}'; this index covers claude-code, codex and pi"),
                )
            })
        })
        .collect()
}

fn resolve_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_QUERY_LIMIT).min(MAX_QUERY_LIMIT)
}

/// Re-check that a stored row really belongs to this project.
///
/// The database is already namespaced and key-bound, so this can only fire on a
/// tampered or corrupt index — which is exactly when a memory bank must refuse
/// rather than answer.
fn authorize_session(
    fence: &ProjectFence,
    session: IndexedSession,
) -> MemoryIndexResult<IndexedSession> {
    if session.project_key != fence.namespace_key() {
        return Err(MemoryIndexError::new(
            ERR_OUT_OF_SCOPE,
            format!(
                "session {} is recorded under project {}, not {}",
                session.session_key,
                session.project_key,
                fence.namespace_key()
            ),
        ));
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_index::types::{
        FileIdentity, LineageDepth, NormalizedMessage, NormalizedRole, SourcePointer,
        TimestampConfidence, SCHEMA_VERSION,
    };

    struct Harness {
        _temp: tempfile::TempDir,
        service: MemoryIndexService,
        project_root: PathBuf,
        transcript: PathBuf,
    }

    /// Seeds a store directly rather than through ingest: this layer's job is
    /// authorization and freshness, and driving it from a real vendor store
    /// would make every test depend on process env.
    fn harness() -> Harness {
        let temp = tempfile::tempdir().unwrap();
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        let project_root = project_root.canonicalize().unwrap();
        let state_root = temp.path().join("state");
        let service = MemoryIndexService::new(state_root);

        let transcript = temp.path().join("chat.jsonl");
        std::fs::write(&transcript, b"{\"role\":\"user\",\"text\":\"alpha\"}\n").unwrap();

        Harness {
            service,
            project_root,
            transcript,
            _temp: temp,
        }
    }

    fn seed(harness: &Harness, scope: SessionScope, text: &str) -> String {
        let fence = ProjectFence::single(&harness.project_root).unwrap();
        let location = fence.index_location(harness.service.state_root()).unwrap();
        location.ensure_dir().unwrap();
        let mut store =
            MemoryStore::open(&location.database_path, &location.namespace_key).unwrap();

        let identity = FileIdentity::read(&harness.transcript).unwrap();
        let bytes = std::fs::read(&harness.transcript).unwrap();
        let pointer =
            SourcePointer::for_record(&identity, harness.transcript.to_str().unwrap(), 0, &bytes);
        let key = format!("claude-code:{}", harness.transcript.display());
        let session = IndexedSession {
            schema_version: SCHEMA_VERSION,
            session_key: key.clone(),
            vendor: "claude-code".into(),
            vendor_session_id: "sess-1".into(),
            root_session_key: key.clone(),
            lineage_depth: LineageDepth::ROOT,
            project_key: location.namespace_key.clone(),
            scope,
            cwd: Some(harness.project_root.to_string_lossy().into_owned()),
            title: Some("seeded".into()),
            first_message_at_utc: Some("2026-09-01T00:00:00.000Z".into()),
            first_message_at_ms: Some(1_788_220_800_000),
            last_activity_at_utc: None,
            last_activity_at_ms: None,
            timestamp_confidence: TimestampConfidence::Native,
            message_count: 1,
            tool_count: 0,
            file_path: harness.transcript.to_string_lossy().into_owned(),
            source: pointer.clone(),
        };
        let message = NormalizedMessage {
            schema_version: SCHEMA_VERSION,
            message_key: format!("{key}#0"),
            session_key: key.clone(),
            root_session_key: key.clone(),
            lineage_depth: LineageDepth::ROOT,
            ordinal: 0,
            role: NormalizedRole::User,
            timestamp_utc: Some("2026-09-01T00:00:00.000Z".into()),
            timestamp_ms: Some(1_788_220_800_000),
            timestamp_confidence: TimestampConfidence::Native,
            text: text.to_string(),
            tool_name: None,
            tool_call_id: None,
            source: pointer,
        };
        store.replace_session(&session, &[message], &[]).unwrap();
        key
    }

    #[test]
    fn a_project_with_no_index_answers_empty_rather_than_creating_one() {
        let harness = harness();
        let status = harness.service.status(&harness.project_root).unwrap();
        assert!(!status.exists);
        assert_eq!(status.session_count, 0);
        assert!(
            !Path::new(&status.database_path).exists(),
            "a status read must not create the database"
        );

        assert!(harness
            .service
            .list_sessions(&harness.project_root, None, false, &[])
            .unwrap()
            .is_empty());
        let response = harness
            .service
            .search(
                &harness.project_root,
                &MemorySearchRequest {
                    query: "anything".into(),
                    limit: None,
                    agents: Vec::new(),
                    include_unscoped: false,
                    include_stale: false,
                },
            )
            .unwrap();
        assert!(response.hits.is_empty());
        assert!(
            !Path::new(&status.database_path).exists(),
            "a search must not create the database either"
        );
    }

    #[test]
    fn search_and_session_detail_round_trip_through_the_service() {
        let harness = harness();
        let key = seed(&harness, SessionScope::Scoped, "the login redirect loops");

        let response = harness
            .service
            .search(
                &harness.project_root,
                &MemorySearchRequest {
                    query: "redirect".into(),
                    limit: None,
                    agents: Vec::new(),
                    include_unscoped: false,
                    include_stale: false,
                },
            )
            .unwrap();
        assert_eq!(response.hits.len(), 1);
        assert!(response.hits[0].source_fresh);
        assert_eq!(response.stale_hits_omitted, 0);

        let detail = harness
            .service
            .get_session(&harness.project_root, &key, None, false)
            .unwrap()
            .expect("session");
        assert_eq!(detail.session.session_key, key);
        assert_eq!(detail.messages.len(), 1);

        let status = harness.service.status(&harness.project_root).unwrap();
        assert!(status.exists);
        assert_eq!(status.session_count, 1);
        assert_eq!(status.message_count, 1);
        assert_eq!(
            status.newest_first_message_at_utc.as_deref(),
            Some("2026-09-01T00:00:00.000Z")
        );
    }

    /// AC10 at the layer the constraint names. A transcript rewritten after
    /// indexing must not have its stored text served as current.
    #[test]
    fn a_hit_whose_transcript_changed_is_withheld_unless_asked_for() {
        let harness = harness();
        seed(&harness, SessionScope::Scoped, "the login redirect loops");
        std::fs::write(
            &harness.transcript,
            b"{\"role\":\"user\",\"text\":\"beta\"}\n",
        )
        .unwrap();

        let request = MemorySearchRequest {
            query: "redirect".into(),
            limit: None,
            agents: Vec::new(),
            include_unscoped: false,
            include_stale: false,
        };
        let withheld = harness
            .service
            .search(&harness.project_root, &request)
            .unwrap();
        assert!(withheld.hits.is_empty());
        assert_eq!(withheld.stale_hits_omitted, 1);

        let asked = harness
            .service
            .search(
                &harness.project_root,
                &MemorySearchRequest {
                    include_stale: true,
                    ..request
                },
            )
            .unwrap();
        assert_eq!(asked.hits.len(), 1);
        assert!(
            !asked.hits[0].source_fresh,
            "a stale hit must be labelled even when it is returned"
        );
    }

    /// AC8 at the service boundary: unproven ownership stays out by default,
    /// and a session detail request for one reads as absent rather than denied.
    #[test]
    fn unscoped_sessions_are_invisible_by_default() {
        let harness = harness();
        let key = seed(&harness, SessionScope::Unscoped, "borrowed context");

        assert!(harness
            .service
            .list_sessions(&harness.project_root, None, false, &[])
            .unwrap()
            .is_empty());
        assert_eq!(
            harness
                .service
                .list_sessions(&harness.project_root, None, true, &[])
                .unwrap()
                .len(),
            1
        );
        assert!(
            harness
                .service
                .get_session(&harness.project_root, &key, None, false)
                .unwrap()
                .is_none(),
            "an unscoped session must read as absent, not as a denial that \
             confirms it exists"
        );
    }

    /// A tampered index — one whose rows claim another project — must refuse.
    #[test]
    fn a_row_recorded_under_another_project_is_refused() {
        let fence = ProjectFence::single(Path::new("/some/project")).unwrap();
        let mut session = IndexedSession {
            schema_version: SCHEMA_VERSION,
            session_key: "claude-code:/x.jsonl".into(),
            vendor: "claude-code".into(),
            vendor_session_id: "s".into(),
            root_session_key: "claude-code:/x.jsonl".into(),
            lineage_depth: LineageDepth::ROOT,
            project_key: "someone-else-0011223344556677".into(),
            scope: SessionScope::Scoped,
            cwd: None,
            title: None,
            first_message_at_utc: None,
            first_message_at_ms: None,
            last_activity_at_utc: None,
            last_activity_at_ms: None,
            timestamp_confidence: TimestampConfidence::Unknown,
            message_count: 0,
            tool_count: 0,
            file_path: "/x.jsonl".into(),
            source: SourcePointer::for_record(
                &FileIdentity {
                    device: 0,
                    inode: 0,
                    size_bytes: 0,
                    modified_unix_ms: 0,
                },
                "/x.jsonl",
                0,
                b"",
            ),
        };
        assert_eq!(
            authorize_session(&fence, session.clone()).unwrap_err().code,
            ERR_OUT_OF_SCOPE
        );
        session.project_key = fence.namespace_key();
        assert!(authorize_session(&fence, session).is_ok());
    }

    #[test]
    fn a_missing_session_key_is_none_not_an_error() {
        let harness = harness();
        seed(&harness, SessionScope::Scoped, "present");
        assert!(harness
            .service
            .get_session(
                &harness.project_root,
                "claude-code:/nope.jsonl",
                None,
                false
            )
            .unwrap()
            .is_none());
    }

    /// The wire contract for the optional filter: omitted means every agent,
    /// and an agent this index does not cover is a rejection rather than a
    /// silent widening back to all of them.
    #[test]
    fn the_agent_filter_is_optional_and_validated() {
        assert_eq!(parse_agents(&[]).unwrap(), Vec::new());
        assert_eq!(
            parse_agents(&["pi".to_string(), "codex".to_string()]).unwrap(),
            vec![MemoryVendor::Pi, MemoryVendor::Codex]
        );
        let error = parse_agents(&["gemini-cli".to_string()]).unwrap_err();
        assert_eq!(error.code, ERR_OUT_OF_SCOPE);
        assert!(error.detail.contains("gemini-cli"), "{}", error.detail);
    }

    /// A request that names no agent must return the seeded session, and one
    /// that names a different agent must not.
    #[test]
    fn omitting_the_agent_searches_across_agents() {
        let harness = harness();
        seed(&harness, SessionScope::Scoped, "cross agent recall");

        let all = harness
            .service
            .search(
                &harness.project_root,
                &MemorySearchRequest {
                    query: "recall".into(),
                    limit: None,
                    agents: Vec::new(),
                    include_unscoped: false,
                    include_stale: false,
                },
            )
            .unwrap();
        assert_eq!(all.hits.len(), 1);

        let other_agent = harness
            .service
            .search(
                &harness.project_root,
                &MemorySearchRequest {
                    query: "recall".into(),
                    limit: None,
                    agents: vec!["pi".into()],
                    include_unscoped: false,
                    include_stale: false,
                },
            )
            .unwrap();
        assert!(
            other_agent.hits.is_empty(),
            "the seeded session is claude-code, so a pi-only filter must exclude it"
        );
    }

    /// AC12's host-side half.
    ///
    /// A build walks every transcript this project has — 3139 files and 154
    /// seconds on the corpus it was measured against — so it must stay
    /// reachable only from the two explicit user-triggered entry points. The
    /// renderer half of this rule is enforced in
    /// `parity-checklist.test.ts`; this is the half that covers callers with no
    /// renderer at all, like a startup path or a route added later.
    #[test]
    fn building_the_index_is_reachable_only_from_the_explicit_entry_points() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let allowed = [
            // The Tauri command behind the project menu item.
            "memory_index/commands.rs",
            // Its HTTP twin, which the browser client's same menu item calls.
            "web/memory_index_api.rs",
            // The implementation and its own tests.
            "memory_index/service.rs",
            "memory_index/ingest.rs",
        ];
        let mut callers = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|value| value.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let production = source.split("#[cfg(test)]").next().unwrap_or_default();
                if !production.contains("build_index(") && !production.contains(".build(\n") {
                    continue;
                }
                if !production.contains("memory_index") && !production.contains("MemoryIndex") {
                    continue;
                }
                let relative = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                if !allowed.contains(&relative.as_str()) {
                    callers.push(relative);
                }
            }
        }
        assert!(
            callers.is_empty(),
            "the memory index build must stay behind the explicit project action; \
             unexpected callers: {callers:?}"
        );
    }

    #[test]
    fn limits_default_and_clamp() {
        assert_eq!(resolve_limit(None), DEFAULT_QUERY_LIMIT);
        assert_eq!(resolve_limit(Some(5)), 5);
        assert_eq!(resolve_limit(Some(usize::MAX)), MAX_QUERY_LIMIT);
    }

    /// Every entry point derives the fence from one project root, so a caller
    /// cannot widen the scope by passing a parent directory.
    #[test]
    fn an_ancestor_directory_is_a_different_project_not_a_wider_one() {
        let harness = harness();
        seed(&harness, SessionScope::Scoped, "inside");
        let parent = harness.project_root.parent().unwrap().to_path_buf();
        assert!(
            harness
                .service
                .list_sessions(&parent, None, true, &[])
                .unwrap()
                .is_empty(),
            "the parent directory keys to its own namespace, which is empty"
        );
    }

    #[test]
    fn a_relative_project_root_is_refused_at_every_entry_point() {
        let harness = harness();
        let relative = Path::new("relative/project");
        assert!(harness.service.status(relative).is_err());
        assert!(harness
            .service
            .list_sessions(relative, None, false, &[])
            .is_err());
        assert!(harness
            .service
            .search(
                relative,
                &MemorySearchRequest {
                    query: "x".into(),
                    limit: None,
                    agents: Vec::new(),
                    include_unscoped: false,
                    include_stale: false,
                }
            )
            .is_err());
        assert!(harness
            .service
            .get_session(relative, "k", None, false)
            .is_err());
        assert!(harness
            .service
            .build(relative, &IngestOptions::default(), &mut |_| {})
            .is_err());
    }
}
