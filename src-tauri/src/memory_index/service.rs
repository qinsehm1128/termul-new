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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::ingest::{self, CancelFlag, IngestOptions, IngestProgress, IngestReport};
use super::paths::MemoryVendor;
use super::scope::ProjectFence;
use super::store::{MemorySearchHit, MemoryStore, MAX_QUERY_LIMIT};
use super::types::{CompactionRecord, IndexedSession, PointerFreshness, SessionScope};
use super::{
    MemoryIndexError, MemoryIndexResult, ERR_BUILD_IN_PROGRESS, ERR_OUT_OF_SCOPE,
    ERR_STORE_FAILED,
};

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
    /// An index exists but was written by an older on-disk layout, so the next
    /// build discards it and reads the whole corpus again.
    ///
    /// Reported without opening the index: opening it would migrate it, and
    /// migrating an outdated layout means dropping it — the user would be told
    /// after the fact about something they could no longer choose to postpone.
    pub needs_rebuild: bool,
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
    /// Project keys with a build running right now.
    ///
    /// The desktop menu item disables itself while a build is in flight, but
    /// that guard lives in one renderer: a second window, the browser client and
    /// the HTTP route can all reach [`Self::build`] at the same time. Two writers
    /// interleaving `replace_session` and `prune_missing` over one SQLite file is
    /// not something WAL and a busy timeout resolve — they turn it into a
    /// five-second stall and then an error, having already half-applied one of
    /// the two prunes. Refusing the second build outright is both cheaper and
    /// the honest answer.
    ///
    /// The map doubles as the cancel registry: the value is the running build's
    /// stop flag, so [`Self::cancel_build`] is a lookup rather than a second
    /// structure that has to be kept in step with this one.
    building: Arc<Mutex<HashMap<String, CancelFlag>>>,
}

impl MemoryIndexService {
    #[must_use]
    pub fn new(state_root: PathBuf) -> Self {
        Self {
            state_root,
            building: Arc::new(Mutex::new(HashMap::new())),
        }
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
        let guard = BuildGuard::acquire(&self.building, fence.namespace_key())?;
        ingest::build_index(&fence, &self.state_root, options, guard.cancel(), progress)
    }

    /// Ask a running build for this project to stop.
    ///
    /// Returns `false` when nothing was running — not an error, because the
    /// build may simply have finished between the user clicking and this call
    /// arriving, and a UI should not have to explain that race.
    ///
    /// Cooperative: the flag is read once per file, so the build stops within
    /// one transcript rather than instantly. Everything already written stays —
    /// each session is its own transaction — and a cancelled build never prunes.
    pub fn cancel_build(&self, project_root: &Path) -> MemoryIndexResult<bool> {
        let fence = ProjectFence::single(project_root)?;
        let key = fence.namespace_key();
        let in_flight = lock(&self.building);
        match in_flight.get(&key) {
            Some(flag) => {
                flag.cancel();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Is a build running for this project right now?
    #[must_use]
    pub fn is_building(&self, project_root: &Path) -> bool {
        let Ok(fence) = ProjectFence::single(project_root) else {
            return false;
        };
        lock(&self.building).contains_key(&fence.namespace_key())
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
            needs_rebuild: false,
        };
        if !status.exists {
            return Ok(status);
        }
        if !MemoryStore::is_current_version(&location.database_path) {
            // Deliberately return before opening. See `MemoryStore::open`.
            status.needs_rebuild = true;
            return Ok(status);
        }
        // `exists` was true a moment ago, so a failure here is a race or a
        // permission problem — not a zero-byte database. Reporting 0 would be an
        // assertion about the index rather than an admission about the stat.
        let metadata = std::fs::metadata(&location.database_path).map_err(|error| {
            MemoryIndexError::new(
                ERR_STORE_FAILED,
                format!(
                    "could not stat index {}: {error}",
                    location.database_path.display()
                ),
            )
        })?;
        status.size_bytes = metadata.len();
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
        let raw_compactions =
            store.search_compactions(&request.query, request.include_unscoped, limit)?;
        let (compactions, stale_compactions) =
            finish_compactions(raw_compactions, request.include_stale);
        let raw = store.search(&request.query, request.include_unscoped, &vendors, limit)?;
        let (hits, stale_hits) = self.finish_hits(&fence, raw, request.include_stale)?;
        Ok(MemorySearchResponse {
            project_key: fence.namespace_key(),
            query: request.query.clone(),
            compactions,
            hits,
            stale_hits_omitted: stale_hits + stale_compactions,
        })
    }

    /// One session and its messages in transcript order.
    pub fn get_session(
        &self,
        project_root: &Path,
        session_key: &str,
        limit: Option<usize>,
        include_stale: bool,
        include_unscoped: bool,
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
        //
        // `include_unscoped` is the same switch `list_sessions` and `search`
        // take, and it is here because the three have to agree. Without it a
        // caller could list an unproven session, get a key back, and then be
        // told that key does not exist.
        if session.scope != SessionScope::Scoped && !include_unscoped {
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

    /// Verify every row's source pointer, dropping the ones that no longer
    /// describe the bytes they were indexed from.
    ///
    /// Row-level *authorization* is deliberately not attempted here and this is
    /// not an omission: a [`MemorySearchHit`] carries no project key, and the
    /// binding it would be checked against is enforced one layer down —
    /// `MemoryStore::open` refuses a database whose recorded `project_key` is not
    /// the one being asked for, so every row this iterates already came from
    /// this project's index. [`authorize_session`] is the row-level check, and it
    /// runs where there is actually a `project_key` to compare.
    ///
    /// (An earlier version asserted `fence.namespace_key() == fence.namespace_key()`
    /// here and called it re-authorization. It could not fail, and it made the
    /// check look present when it was not.)
    fn finish_hits(
        &self,
        _fence: &ProjectFence,
        raw: Vec<MemorySearchHit>,
        include_stale: bool,
    ) -> MemoryIndexResult<(Vec<MemorySearchHit>, u32)> {
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

/// Holds one project's build slot for as long as the build runs.
///
/// A guard rather than a bare insert/remove pair so the slot is released on
/// every exit path, including an error partway through a two-minute walk.
struct BuildGuard {
    building: Arc<Mutex<HashMap<String, CancelFlag>>>,
    project_key: String,
    cancel: CancelFlag,
}

impl BuildGuard {
    fn acquire(
        building: &Arc<Mutex<HashMap<String, CancelFlag>>>,
        project_key: String,
    ) -> MemoryIndexResult<Self> {
        let cancel = CancelFlag::new();
        {
            let mut in_flight = lock(building);
            if in_flight.contains_key(&project_key) {
                return Err(MemoryIndexError::new(
                    ERR_BUILD_IN_PROGRESS,
                    "a memory index build is already running for this project",
                ));
            }
            in_flight.insert(project_key.clone(), cancel.clone());
        }
        Ok(Self {
            building: Arc::clone(building),
            project_key,
            cancel,
        })
    }

    fn cancel(&self) -> &CancelFlag {
        &self.cancel
    }
}

impl Drop for BuildGuard {
    fn drop(&mut self) {
        lock(&self.building).remove(&self.project_key);
    }
}

/// A panic in a previous build must not make the feature permanently
/// unavailable; the map is plain data with no invariant a panic could break.
fn lock(
    building: &Arc<Mutex<HashMap<String, CancelFlag>>>,
) -> std::sync::MutexGuard<'_, HashMap<String, CancelFlag>> {
    building.lock().unwrap_or_else(|poisoned| {
        building.clear_poison();
        poisoned.into_inner()
    })
}

/// Apply the same freshness gate to compaction summaries that every other row
/// goes through.
///
/// They used to skip it entirely and be returned unconditionally, which made the
/// module's own promise — "every returned hit has its source pointer verified" —
/// false for exactly the rows where it matters most: a compaction is the agent's
/// own summary of a whole stretch of context, so serving one from a transcript
/// that has since been rewritten is the most confidently wrong answer this
/// service can give.
fn finish_compactions(
    raw: Vec<CompactionRecord>,
    include_stale: bool,
) -> (Vec<CompactionRecord>, u32) {
    if include_stale {
        return (raw, 0);
    }
    let mut kept = Vec::with_capacity(raw.len());
    let mut omitted = 0;
    for record in raw {
        if Path::new(&record.source.file_path).is_absolute()
            && record.source.verify() == PointerFreshness::Fresh
        {
            kept.push(record);
        } else {
            omitted += 1;
        }
    }
    (kept, omitted)
}

/// Resolve the caller's limit into the range the store will actually honor.
///
/// Clamped identically to `store::clamp_limit`, so `limit: 0` means one row in
/// both places rather than zero here and one there. Zero rows is not a request
/// this API can express, and silently turning it into "one row" at the SQL layer
/// while reporting it as "zero" at this one is worse than being consistent.
fn resolve_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .clamp(1, MAX_QUERY_LIMIT)
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
        store.replace_session(&session, &[message], &[], 0,).unwrap();
        key
    }

    /// Seed one compaction whose source file is then rewritten underneath it.
    fn seed_compaction(harness: &Harness, summary: &str) {
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
        let record = CompactionRecord {
            schema_version: SCHEMA_VERSION,
            session_key: key.clone(),
            root_session_key: key,
            ordinal: 0,
            summary: summary.to_string(),
            tokens_before: Some(120_000),
            first_kept_entry_id: None,
            timestamp_utc: Some("2026-09-01T00:00:00.000Z".into()),
            timestamp_ms: Some(1_788_220_800_000),
            source: pointer,
        };
        let existing = store.get_session(&record.session_key).unwrap();
        let session = existing.expect("seed the session first");
        store.replace_session(&session, &[], &[record], 0,).unwrap();
    }

    fn search_for(harness: &Harness, query: &str, include_stale: bool) -> MemorySearchResponse {
        harness
            .service
            .search(
                &harness.project_root,
                &MemorySearchRequest {
                    query: query.into(),
                    limit: None,
                    agents: Vec::new(),
                    include_unscoped: false,
                    include_stale,
                },
            )
            .unwrap()
    }

    /// A compaction is the agent's own summary of a whole stretch of context.
    /// Serving one out of a transcript that has since been rewritten is the most
    /// confidently wrong answer this service can produce, and compactions used
    /// to bypass the freshness gate that every other row goes through.
    #[test]
    fn a_stale_compaction_is_withheld_like_any_other_row() {
        let harness = harness();
        seed(&harness, SessionScope::Scoped, "the login redirect loops");
        seed_compaction(&harness, "summarised the failed migration attempt");

        let fresh = search_for(&harness, "migration", false);
        assert_eq!(fresh.compactions.len(), 1, "the seeded compaction is fresh");
        assert_eq!(fresh.stale_hits_omitted, 0);

        // Rewrite the transcript so the recorded byte range no longer hashes.
        std::fs::write(&harness.transcript, b"{\"role\":\"user\",\"text\":\"REWRITTEN\"}\n")
            .unwrap();

        let after = search_for(&harness, "migration", false);
        assert!(
            after.compactions.is_empty(),
            "a stale compaction was returned anyway: {:?}",
            after.compactions
        );
        assert!(
            after.stale_hits_omitted >= 1,
            "the withheld compaction was not counted"
        );

        // And it is still reachable when the caller explicitly asks for stale.
        let forced = search_for(&harness, "migration", true);
        assert_eq!(forced.compactions.len(), 1);
    }

    /// The three read paths have to agree about unproven sessions: a listing
    /// that hands out a key the detail call then calls absent is a dead end the
    /// UI cannot recover from.
    #[test]
    fn an_unscoped_session_is_openable_on_the_same_terms_it_was_listed() {
        let harness = harness();
        let key = seed(&harness, SessionScope::Unscoped, "unproven ownership");

        // Default: not listed, not openable.
        assert!(harness
            .service
            .list_sessions(&harness.project_root, None, false, &[])
            .unwrap()
            .is_empty());
        assert!(harness
            .service
            .get_session(&harness.project_root, &key, None, false, false)
            .unwrap()
            .is_none());

        // Asked for: listed, and openable on the same switch.
        assert_eq!(
            harness
                .service
                .list_sessions(&harness.project_root, None, true, &[])
                .unwrap()
                .len(),
            1
        );
        let detail = harness
            .service
            .get_session(&harness.project_root, &key, None, false, true)
            .unwrap();
        assert!(
            detail.is_some(),
            "listed with include_unscoped but not openable with it"
        );
    }

    /// The renderer disables its menu item during a build, but that guard lives
    /// in one window; the HTTP route and the browser client reach the same
    /// service. Two builds interleaving their `prune_missing` passes is data
    /// loss, so the second one is refused rather than queued.
    #[test]
    fn a_second_concurrent_build_of_one_project_is_refused() {
        let harness = harness();
        let fence = ProjectFence::single(&harness.project_root).unwrap();
        let key = fence.namespace_key();
        let guard = BuildGuard::acquire(&harness.service.building, key.clone()).unwrap();

        let error = harness
            .service
            .build(&harness.project_root, &IngestOptions::default(), &mut |_| {})
            .unwrap_err();
        assert_eq!(error.code, ERR_BUILD_IN_PROGRESS, "{error}");

        // Releasing the slot makes the project buildable again — including after
        // a build that failed partway through.
        drop(guard);
        assert!(harness
            .service
            .build(&harness.project_root, &IngestOptions::default(), &mut |_| {})
            .is_ok());
    }

    /// Two different projects are two different databases and must not block
    /// each other.
    #[test]
    fn a_build_of_one_project_does_not_block_another() {
        let harness = harness();
        let other = harness.project_root.parent().unwrap().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let fence = ProjectFence::single(&harness.project_root).unwrap();
        let _guard = BuildGuard::acquire(&harness.service.building, fence.namespace_key()).unwrap();
        assert!(harness
            .service
            .build(&other, &IngestOptions::default(), &mut |_| {})
            .is_ok());
    }

    /// Asking whether an index needs rebuilding must not be the thing that
    /// destroys it. `MemoryStore::open` migrates, and migrating an outdated
    /// layout drops it — so `status` has to answer without opening.
    #[test]
    fn asking_about_a_stale_index_does_not_discard_it() {
        let harness = harness();
        seed(&harness, SessionScope::Scoped, "the login redirect loops");
        let fence = ProjectFence::single(&harness.project_root).unwrap();
        let location = fence.index_location(harness.service.state_root()).unwrap();

        // Fresh index: current, nothing to rebuild.
        let status = harness.service.status(&harness.project_root).unwrap();
        assert!(status.exists);
        assert!(!status.needs_rebuild);
        assert_eq!(status.session_count, 1);

        // Now make it look like it came from an older layout.
        {
            let connection = rusqlite::Connection::open(&location.database_path).unwrap();
            connection
                .execute(
                    "UPDATE meta SET value = '1' WHERE key = 'store_version'",
                    [],
                )
                .unwrap();
        }

        let status = harness.service.status(&harness.project_root).unwrap();
        assert!(status.needs_rebuild, "a stale layout was not reported");
        assert!(status.exists);

        // And the rows are still there — the question did not answer itself by
        // deleting the subject.
        let store = MemoryStore::open_in_memory("probe").unwrap();
        drop(store);
        let connection = rusqlite::Connection::open_with_flags(
            &location.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let sessions: i64 = connection
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 1, "the stale index was dropped just by asking");
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
            .get_session(&harness.project_root, &key, None, false, false,)
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
                .get_session(&harness.project_root, &key, None, false, false,)
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
                false,
                false,
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
            .get_session(relative, "k", None, false, false,)
            .is_err());
        assert!(harness
            .service
            .build(relative, &IngestOptions::default(), &mut |_| {})
            .is_err());
    }
}
