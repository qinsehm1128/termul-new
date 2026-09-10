//! Discovery and the build pipeline.
//!
//! `discover → gate → adapt → redact → upsert`, per vendor, for one project.
//!
//! ## Why this does not reuse `cli_session`'s scanner
//!
//! Three of that scanner's decisions are correct for a session picker and wrong
//! here, and every one of them would silently produce a worse index:
//!
//! * it skips `subagents/` — this index flattens subagent transcripts on
//!   purpose, and on the measured machine they are 63 of this project's 77
//!   Claude files;
//! * it caps at 80 sessions per agent and 1000 walked files — a memory bank
//!   truncated to the 80 most recent sessions is not a memory bank;
//! * it sorts by file mtime — the ordering this feature exists to replace.
//!
//! ## Only when asked
//!
//! [`build_index`] is reachable from exactly one place in the product: the
//! project's right-click menu. It is never called on startup, on panel mount, or
//! from a list request. That is what keeps a 55 GB corpus with a 2.6 GB single
//! file from being something the app does to you.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use super::adapters::{self, AdaptedTranscript, AdapterIssue, ISSUE_COMPACT_FAILED};
use super::paths::{vendor_scan_roots, vendor_store_roots, IndexLocation, MemoryVendor};
use super::scope::ProjectFence;
use super::store::MemoryStore;
use super::types::{FileIdentity, SessionScope};
use super::{MemoryIndexError, MemoryIndexResult, ERR_INGEST_FAILED};

/// Cooperative stop signal for one build.
///
/// A build walks tens of thousands of files and took 154 s on the measured
/// corpus, so "start it and wait" is not an acceptable only option. The flag is
/// checked once per file: fine-grained enough that a cancel lands within one
/// transcript, coarse enough to cost nothing.
#[derive(Debug, Default, Clone)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Deepest directory nesting followed while walking a vendor store.
///
/// pi reaches 4 levels of transcript nesting and Codex partitions by
/// year/month/day; 12 clears both with room to spare while still bounding a
/// pathological symlink-free cycle.
const MAX_WALK_DEPTH: usize = 12;

/// Upper bound on transcripts examined per vendor in one build.
///
/// Generous rather than tight: the point of this index is completeness, and the
/// cap exists only so a corrupt or shared store cannot make one build unbounded.
pub const MAX_FILES_PER_VENDOR: usize = 50_000;

/// Knobs for one build.
#[derive(Debug, Clone)]
pub struct IngestOptions {
    /// Re-read every transcript instead of skipping ones whose file identity is
    /// unchanged.
    pub full_rebuild: bool,
    /// Index sessions whose project ownership could not be proven. Off by
    /// default; they are still excluded from query results either way.
    pub index_unscoped: bool,
    pub max_files_per_vendor: usize,
}

impl Default for IngestOptions {
    fn default() -> Self {
        Self {
            full_rebuild: false,
            index_unscoped: false,
            max_files_per_vendor: MAX_FILES_PER_VENDOR,
        }
    }
}

/// Progress for a long build. Reported per file so a UI can show motion on a
/// corpus that takes minutes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestProgress {
    /// Which project this build belongs to. Carried in the event so a UI with
    /// several projects open can route it without tracking a request id.
    pub project_key: String,
    pub vendor: String,
    pub files_seen: u32,
    pub files_total: u32,
    pub sessions_indexed: u32,
}

/// What one build did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestReport {
    pub project_key: String,
    pub database_path: String,
    pub files_scanned: u32,
    pub sessions_indexed: u32,
    pub sessions_skipped_unchanged: u32,
    pub sessions_forgotten: u32,
    pub sessions_out_of_scope: u32,
    pub messages_indexed: u32,
    pub compactions_indexed: u32,
    pub duration_ms: u64,
    /// The caller asked to stop before the walk finished.
    ///
    /// Everything already written is complete and usable — each session is its
    /// own transaction — but the index is *partial*, which is why a cancelled
    /// build never prunes (see [`build_index`]).
    pub cancelled: bool,
    pub issues: Vec<AdapterIssue>,
}

/// Build (or refresh) one project's index.
///
/// `state_root` is injected by the host — the desktop names it from Tauri's
/// `app_data_dir()`, the standalone server from its service-account state dir,
/// and the two must never be the same tree.
pub fn build_index(
    fence: &ProjectFence,
    state_root: &Path,
    options: &IngestOptions,
    cancel: &CancelFlag,
    progress: &mut dyn FnMut(IngestProgress),
) -> MemoryIndexResult<IngestReport> {
    let started = Instant::now();
    let location = fence.index_location(state_root)?;
    location.ensure_dir()?;
    let mut store = MemoryStore::open(&location.database_path, &location.namespace_key)?;

    let mut report = IngestReport {
        project_key: location.namespace_key.clone(),
        database_path: location.database_path.to_string_lossy().into_owned(),
        ..IngestReport::default()
    };
    let mut seen_keys: Vec<String> = Vec::new();
    let mut scanned_vendors: Vec<MemoryVendor> = Vec::new();

    ingest_claude(
        fence,
        &location,
        options,
        cancel,
        &mut store,
        &mut report,
        &mut seen_keys,
        &mut scanned_vendors,
        progress,
    )?;
    ingest_pi(
        fence,
        &location,
        options,
        cancel,
        &mut store,
        &mut report,
        &mut seen_keys,
        &mut scanned_vendors,
        progress,
    )?;
    ingest_codex(
        fence,
        &location,
        options,
        cancel,
        &mut store,
        &mut report,
        &mut seen_keys,
        &mut scanned_vendors,
        progress,
    )?;

    // A cancelled build must not prune. `seen_keys` only lists what the walk
    // reached, so pruning against it would read every transcript the walk never
    // got to as deleted and erase it from the index — turning "stop early" into
    // "throw most of it away".
    report.cancelled = cancel.is_cancelled();
    if !report.cancelled {
        report.sessions_forgotten = prune_missing(&mut store, &seen_keys, &scanned_vendors)?;
        // Only on a complete build: compaction costs real time, and paying it
        // for a partial index that the next build will rewrite anyway is waste.
        if let Err(error) = store.compact() {
            report.issues.push(AdapterIssue::new(
                ISSUE_COMPACT_FAILED,
                &location.database_path,
                error.detail,
            ));
        }
    }
    report.duration_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// Drop indexed sessions that no longer exist on disk.
///
/// Restricted to vendors that were actually scanned this run. Without that
/// guard, a vendor whose store is momentarily unreachable — `CODEX_HOME`
/// pointing somewhere else, an unmounted home — would look like "every Codex
/// session was deleted" and the build would erase them all.
fn prune_missing(
    store: &mut MemoryStore,
    seen_keys: &[String],
    scanned_vendors: &[MemoryVendor],
) -> MemoryIndexResult<u32> {
    if scanned_vendors.is_empty() {
        return Ok(0);
    }
    let stale: Vec<String> = store
        .all_session_keys()?
        .into_iter()
        .filter(|key| {
            scanned_vendors
                .iter()
                .any(|vendor| key.starts_with(&format!("{}:", vendor.as_str())))
                && !seen_keys.iter().any(|seen| seen == key)
        })
        .collect();
    store.forget_sessions(&stale)
}

#[allow(clippy::too_many_arguments)]
fn ingest_claude(
    fence: &ProjectFence,
    location: &IndexLocation,
    options: &IngestOptions,
    cancel: &CancelFlag,
    store: &mut MemoryStore,
    report: &mut IngestReport,
    seen_keys: &mut Vec<String>,
    scanned_vendors: &mut Vec<MemoryVendor>,
    progress: &mut dyn FnMut(IngestProgress),
) -> MemoryIndexResult<()> {
    let roots: Vec<PathBuf> = vendor_scan_roots(fence.project())
        .into_iter()
        .filter(|root| root.vendor == MemoryVendor::ClaudeCode)
        .map(|root| root.path)
        .collect();
    if roots.is_empty() {
        return Ok(());
    }
    scanned_vendors.push(MemoryVendor::ClaudeCode);

    let mut files = Vec::new();
    for root in &roots {
        walk(root, options.max_files_per_vendor, &mut |path| {
            if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
                files.push(path.to_path_buf());
            }
        });
    }
    // Roots first: a sidechain's `sessionId` names its parent, so the parent has
    // to be in the map before the child is adapted. A file under `subagents/` is
    // never a root.
    files.sort_by_key(|path| is_under_named_dir(path, "subagents"));
    let total = files.len() as u32;
    let mut roots_by_vendor_id: HashMap<String, String> = HashMap::new();

    for (index, path) in files.iter().enumerate() {
        if cancel.is_cancelled() {
            return Ok(());
        }
        report.files_scanned += 1;
        progress(IngestProgress {
            project_key: location.namespace_key.clone(),
            vendor: MemoryVendor::ClaudeCode.as_str().to_string(),
            files_seen: index as u32 + 1,
            files_total: total,
            sessions_indexed: report.sessions_indexed,
        });
        if !is_inside_vendor_store(MemoryVendor::ClaudeCode, path) {
            continue;
        }
        let key = adapters::session_key(MemoryVendor::ClaudeCode, path);
        seen_keys.push(key.clone());
        let Some(identity) = read_identity(path) else {
            continue;
        };
        if skip_unchanged(store, &key, &identity, options)? {
            report.sessions_skipped_unchanged += 1;
            // A skipped root must still be resolvable as a parent — but only a
            // root. A sidechain's `vendor_session_id` is its *parent's* id, so
            // registering one here would file the child under the parent's key
            // and make every later sidechain of that conversation resolve its
            // root to a sibling. The non-skip path below checks `is_root()` for
            // exactly this reason; the two have to agree.
            if let Some(session) = store
                .get_session(&key)?
                .filter(|session| session.lineage_depth.is_root())
            {
                roots_by_vendor_id
                    .entry(session.vendor_session_id)
                    .or_insert(key);
            }
            continue;
        }
        // The folder name already encodes this project, so ownership is proven
        // without reading the body.
        let adapted = adapters::claude::adapt(
            path,
            &identity,
            &location.namespace_key,
            SessionScope::Scoped,
            &|parent: &str| roots_by_vendor_id.get(parent).cloned(),
        );
        if let Some(session) = adapted.session.as_ref() {
            if session.lineage_depth.is_root() {
                roots_by_vendor_id
                    .entry(session.vendor_session_id.clone())
                    .or_insert_with(|| key.clone());
            }
        }
        commit(store, adapted, report)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn ingest_pi(
    fence: &ProjectFence,
    location: &IndexLocation,
    options: &IngestOptions,
    cancel: &CancelFlag,
    store: &mut MemoryStore,
    report: &mut IngestReport,
    seen_keys: &mut Vec<String>,
    scanned_vendors: &mut Vec<MemoryVendor>,
    progress: &mut dyn FnMut(IngestProgress),
) -> MemoryIndexResult<()> {
    let roots: Vec<PathBuf> = vendor_scan_roots(fence.project())
        .into_iter()
        .filter(|root| root.vendor == MemoryVendor::Pi)
        .map(|root| root.path)
        .collect();
    if roots.is_empty() {
        return Ok(());
    }
    scanned_vendors.push(MemoryVendor::Pi);

    for root in &roots {
        let mut files = Vec::new();
        walk(root, options.max_files_per_vendor, &mut |path| {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            // Filename pattern, not extension: `permissions.jsonl` lives in these
            // same directories and is not a transcript.
            if adapters::pi::is_transcript_name(name) {
                files.push(path.to_path_buf());
            }
        });
        let total = files.len() as u32;
        for (index, path) in files.iter().enumerate() {
            if cancel.is_cancelled() {
                return Ok(());
            }
            report.files_scanned += 1;
            progress(IngestProgress {
                project_key: location.namespace_key.clone(),
                vendor: MemoryVendor::Pi.as_str().to_string(),
                files_seen: index as u32 + 1,
                files_total: total,
                sessions_indexed: report.sessions_indexed,
            });
            if !is_inside_vendor_store(MemoryVendor::Pi, path) {
                continue;
            }
            let key = adapters::session_key(MemoryVendor::Pi, path);
            seen_keys.push(key.clone());
            let Some(identity) = read_identity(path) else {
                continue;
            };
            if skip_unchanged(store, &key, &identity, options)? {
                report.sessions_skipped_unchanged += 1;
                continue;
            }
            let depth = adapters::pi::depth_from_path(root, path);
            let root_key = adapters::pi::root_file_for(root, path)
                .map(|file| adapters::session_key(MemoryVendor::Pi, &file));
            let adapted = adapters::pi::adapt(
                path,
                &identity,
                &location.namespace_key,
                SessionScope::Scoped,
                depth,
                root_key,
            );
            commit(store, adapted, report)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn ingest_codex(
    fence: &ProjectFence,
    location: &IndexLocation,
    options: &IngestOptions,
    cancel: &CancelFlag,
    store: &mut MemoryStore,
    report: &mut IngestReport,
    seen_keys: &mut Vec<String>,
    scanned_vendors: &mut Vec<MemoryVendor>,
    progress: &mut dyn FnMut(IngestProgress),
) -> MemoryIndexResult<()> {
    let roots = vendor_store_roots(MemoryVendor::Codex);
    let roots: Vec<PathBuf> = roots.into_iter().filter(|root| root.is_dir()).collect();
    if roots.is_empty() {
        return Ok(());
    }
    scanned_vendors.push(MemoryVendor::Codex);

    let mut files = Vec::new();
    for root in &roots {
        walk(root, options.max_files_per_vendor, &mut |path| {
            if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
                files.push(path.to_path_buf());
            }
        });
    }
    let total = files.len() as u32;

    // Codex is the one store with no per-project directory, so ownership can
    // only come from each file's own `session_meta.payload.cwd`. Reading just
    // the first line of every file is the price of that.
    struct CodexEntry {
        path: PathBuf,
        key: String,
        meta: adapters::codex::CodexMeta,
        scope: SessionScope,
    }
    let mut entries: Vec<CodexEntry> = Vec::new();
    for (index, path) in files.iter().enumerate() {
        if cancel.is_cancelled() {
            return Ok(());
        }
        report.files_scanned += 1;
        progress(IngestProgress {
            project_key: location.namespace_key.clone(),
            vendor: MemoryVendor::Codex.as_str().to_string(),
            files_seen: index as u32 + 1,
            files_total: total,
            sessions_indexed: report.sessions_indexed,
        });
        if !is_inside_vendor_store(MemoryVendor::Codex, path) {
            continue;
        }
        // Recorded before the file is read, exactly as the other two vendors do
        // it. `seen_keys` answers "is this transcript still on disk", and
        // `read_meta` folds a transient I/O failure into the same `None` as "not
        // a Codex record" — so reading first would let one unreadable moment
        // delete a session from the index that is still there.
        let key = adapters::session_key(MemoryVendor::Codex, path);
        seen_keys.push(key.clone());
        let Some(meta) = adapters::codex::read_meta(path) else {
            continue;
        };
        let scope = fence.classify_cwd(meta.cwd.as_deref());
        if scope == SessionScope::Unscoped {
            report.sessions_out_of_scope += 1;
            if !options.index_unscoped {
                continue;
            }
        }
        entries.push(CodexEntry {
            key,
            path: path.clone(),
            meta,
            scope,
        });
    }

    // thread id -> (session key, parent thread id). Built over the in-scope set
    // so a depth-2 or depth-3 child can be walked back to its root.
    let threads: HashMap<String, (String, Option<String>)> = entries
        .iter()
        .map(|entry| {
            (
                entry.meta.session_id.clone(),
                (entry.key.clone(), entry.meta.parent_thread_id.clone()),
            )
        })
        .collect();
    let resolve_root = |thread: &str| resolve_codex_root(&threads, thread);

    for entry in &entries {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let Some(identity) = read_identity(&entry.path) else {
            continue;
        };
        if skip_unchanged(store, &entry.key, &identity, options)? {
            report.sessions_skipped_unchanged += 1;
            continue;
        }
        let adapted = adapters::codex::adapt(
            &entry.path,
            &identity,
            &location.namespace_key,
            entry.scope,
            &entry.meta,
            &resolve_root,
        );
        commit(store, adapted, report)?;
    }
    Ok(())
}

/// Walk `parent_thread_id` links to the topmost known thread.
///
/// Bounded: the deepest measured spawn is 3, and a cycle in vendor data must not
/// hang a build.
fn resolve_codex_root(
    threads: &HashMap<String, (String, Option<String>)>,
    start: &str,
) -> Option<String> {
    const MAX_HOPS: usize = 8;
    let mut current = start.to_string();
    let mut key = None;
    for _ in 0..MAX_HOPS {
        let (session_key, parent) = threads.get(&current)?;
        key = Some(session_key.clone());
        match parent {
            Some(parent) if *parent != current => current = parent.clone(),
            _ => break,
        }
    }
    key
}

fn commit(
    store: &mut MemoryStore,
    adapted: AdaptedTranscript,
    report: &mut IngestReport,
) -> MemoryIndexResult<()> {
    report.issues.extend(adapted.issues);
    let Some(session) = adapted.session else {
        return Ok(());
    };
    let written = store.replace_session(&session, &adapted.messages, &adapted.compactions)?;
    report.sessions_indexed += written.sessions_written;
    report.messages_indexed += written.messages_written;
    report.compactions_indexed += written.compactions_written;
    Ok(())
}

fn read_identity(path: &Path) -> Option<FileIdentity> {
    FileIdentity::read(path).ok()
}

fn skip_unchanged(
    store: &MemoryStore,
    key: &str,
    identity: &FileIdentity,
    options: &IngestOptions,
) -> MemoryIndexResult<bool> {
    if options.full_rebuild {
        return Ok(false);
    }
    Ok(store
        .indexed_file_identity(key)?
        .is_some_and(|stored| stored.matches(identity)))
}

/// The same four checks `cli_session` applies before touching a transcript:
/// absolute, no `..`, not a symlink, and under a vendor store root.
///
/// Re-implemented here rather than shared because it keys on this module's
/// three-vendor enum; the shape of the rule is deliberately identical.
#[must_use]
pub fn is_inside_vendor_store(vendor: MemoryVendor, path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return false;
    }
    if std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
    {
        return false;
    }
    vendor_store_roots(vendor)
        .iter()
        .any(|root| path.starts_with(root))
}

/// Depth-bounded, symlink-free directory walk.
fn walk(root: &Path, limit: usize, visit: &mut dyn FnMut(&Path)) {
    fn inner(
        dir: &Path,
        limit: usize,
        depth: usize,
        count: &mut usize,
        visit: &mut dyn FnMut(&Path),
    ) {
        if depth > MAX_WALK_DEPTH || *count >= limit {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if *count >= limit {
                return;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // A symlink could point outside the vendor store entirely.
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                inner(&path, limit, depth + 1, count, visit);
            } else if file_type.is_file() {
                *count += 1;
                visit(&path);
            }
        }
    }
    if !root.is_dir() {
        return;
    }
    let mut count = 0;
    inner(root, limit, 0, &mut count, visit);
}

fn is_under_named_dir(path: &Path, name: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str() == name)
}

/// Wrap an ingest failure with the shared code.
pub fn ingest_error(detail: impl Into<String>) -> MemoryIndexError {
    MemoryIndexError::new(ERR_INGEST_FAILED, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_index::types::LineageDepth;
    use std::ffi::OsString;

    /// Vendor roots come from process env via `cli_session::paths`, so the
    /// end-to-end tests have to serialize and restore.
    struct VendorEnv {
        previous: Vec<(&'static str, Option<OsString>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl VendorEnv {
        fn set(claude_config: &Path, pi: &Path, codex: &Path) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let keys = ["CLAUDE_CONFIG_DIR", "PI_CODING_AGENT_DIR", "CODEX_HOME"];
            let previous = keys
                .iter()
                .map(|key| (*key, std::env::var_os(key)))
                .collect();
            std::env::set_var("CLAUDE_CONFIG_DIR", claude_config);
            std::env::set_var("PI_CODING_AGENT_DIR", pi);
            std::env::set_var("CODEX_HOME", codex);
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for VendorEnv {
        fn drop(&mut self) {
            for (key, value) in &self.previous {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        _env: VendorEnv,
        state_root: PathBuf,
        project_root: PathBuf,
        claude_project_dir: PathBuf,
        pi_project_dir: PathBuf,
        codex_sessions: PathBuf,
    }

    fn fixture() -> Fixture {
        use crate::cli_session::paths as vendor_paths;
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let project_root = base.join("workspace").join("termul");
        std::fs::create_dir_all(&project_root).unwrap();
        // Canonicalize so the fixture keys the same way the fence will.
        let project_root = project_root.canonicalize().unwrap();

        let claude_config = base.join("claude-config");
        let claude_project_dir = claude_config
            .join("projects")
            .join(vendor_paths::encode_claude_project_dir(&project_root));
        let pi_root = base.join("pi-sessions");
        let pi_project_dir = pi_root.join(vendor_paths::encode_pi_project_dir(&project_root));
        let codex_home = base.join("codex-home");
        let codex_sessions = codex_home.join("sessions").join("2026").join("09");
        for dir in [&claude_project_dir, &pi_project_dir, &codex_sessions] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let env = VendorEnv::set(&claude_config, &pi_root, &codex_home);
        Fixture {
            state_root: base.join("host-state"),
            project_root,
            claude_project_dir,
            pi_project_dir,
            codex_sessions,
            _env: env,
            _temp: temp,
        }
    }

    fn write(path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
    }

    fn claude_lines(session_id: &str, sidechain: bool, at: &str, text: &str) -> Vec<String> {
        vec![format!(
            r#"{{"type":"user","sessionId":"{session_id}","cwd":"/repo","isSidechain":{sidechain},"timestamp":"{at}","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )]
    }

    fn pi_lines(session_id: &str, at: &str, text: &str) -> Vec<String> {
        vec![
            format!(
                r#"{{"type":"session","version":3,"id":"{session_id}","timestamp":"{at}","cwd":"/repo"}}"#
            ),
            format!(
                r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"{at}","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
            ),
        ]
    }

    fn codex_lines(session_id: &str, cwd: &str, at: &str, text: &str) -> Vec<String> {
        vec![
            format!(
                r#"{{"type":"session_meta","timestamp":"{at}","payload":{{"id":"{session_id}","cwd":"{cwd}","timestamp":"{at}","source":"exec"}}}}"#
            ),
            format!(
                r#"{{"type":"response_item","timestamp":"{at}","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
            ),
        ]
    }

    fn build(fixture: &Fixture, options: IngestOptions) -> IngestReport {
        build_with_cancel(fixture, options, &CancelFlag::new())
    }

    fn build_with_cancel(
        fixture: &Fixture,
        options: IngestOptions,
        cancel: &CancelFlag,
    ) -> IngestReport {
        let fence = ProjectFence::single(&fixture.project_root).unwrap();
        build_index(&fence, &fixture.state_root, &options, cancel, &mut |_| {}).unwrap()
    }

    fn open_store(fixture: &Fixture) -> MemoryStore {
        let fence = ProjectFence::single(&fixture.project_root).unwrap();
        let location = fence.index_location(&fixture.state_root).unwrap();
        MemoryStore::open(&location.database_path, &location.namespace_key).unwrap()
    }

    /// AC1 + AC4, end to end across all three vendors: the index lands
    /// host-private and sessions come back ordered by first message time.
    #[test]
    fn a_build_indexes_all_three_vendors_and_orders_by_first_message_time() {
        let fixture = fixture();
        write(
            &fixture.claude_project_dir.join("c1.jsonl"),
            &claude_lines(
                "sess-c1",
                false,
                "2026-09-02T00:00:00.000Z",
                "claude middle",
            ),
        );
        write(
            &fixture
                .pi_project_dir
                .join("2026-09-03T00-00-00-000Z_aaaaaaaa.jsonl"),
            &pi_lines("pi-1", "2026-09-03T00:00:00.000Z", "pi newest"),
        );
        write(
            &fixture.codex_sessions.join("rollout-1.jsonl"),
            &codex_lines(
                "codex-1",
                &fixture.project_root.to_string_lossy(),
                "2026-09-01T00:00:00.000Z",
                "codex oldest",
            ),
        );

        let report = build(&fixture, IngestOptions::default());
        assert_eq!(report.sessions_indexed, 3, "issues: {:?}", report.issues);
        assert!(
            report
                .database_path
                .starts_with(&fixture.state_root.to_string_lossy().into_owned()),
            "the index must live under the host state root, got {}",
            report.database_path
        );
        assert!(
            !report
                .database_path
                .starts_with(&fixture.project_root.to_string_lossy().into_owned()),
            "the index must never land inside the project"
        );

        let store = open_store(&fixture);
        let vendors: Vec<String> = store
            .list_sessions(false, &[], 10)
            .unwrap()
            .into_iter()
            .map(|session| session.vendor)
            .collect();
        assert_eq!(vendors, vec!["pi", "claude-code", "codex"]);
    }

    /// AC8's ingest half. Codex has no per-project directory, so a session
    /// belonging to another project must be rejected by its own recorded cwd.
    #[test]
    fn a_codex_session_from_another_project_is_not_indexed() {
        let fixture = fixture();
        write(
            &fixture.codex_sessions.join("mine.jsonl"),
            &codex_lines(
                "mine",
                &fixture.project_root.to_string_lossy(),
                "2026-09-01T00:00:00.000Z",
                "belongs here",
            ),
        );
        write(
            &fixture.codex_sessions.join("theirs.jsonl"),
            &codex_lines(
                "theirs",
                "/somewhere/else",
                "2026-09-01T00:00:00.000Z",
                "belongs elsewhere",
            ),
        );

        let report = build(&fixture, IngestOptions::default());
        assert_eq!(report.sessions_indexed, 1);
        assert_eq!(report.sessions_out_of_scope, 1);

        let store = open_store(&fixture);
        assert!(store.search("belongs here", false, &[], 10).unwrap().len() == 1);
        assert!(
            store
                .search("belongs elsewhere", true, &[], 10)
                .unwrap()
                .is_empty(),
            "another project's transcript must not be in this index at all"
        );
    }

    /// The scanner this replaces skips `subagents/`. Flattening them is the
    /// whole point, and on the measured machine they are most of the corpus.
    #[test]
    fn claude_subagent_transcripts_are_flattened_in_with_their_parent() {
        let fixture = fixture();
        write(
            &fixture.claude_project_dir.join("root.jsonl"),
            &claude_lines("sess-root", false, "2026-09-01T00:00:00.000Z", "root work"),
        );
        write(
            &fixture
                .claude_project_dir
                .join("root")
                .join("subagents")
                .join("agent-a.jsonl"),
            &claude_lines(
                "sess-root",
                true,
                "2026-09-01T00:10:00.000Z",
                "subagent work",
            ),
        );
        write(
            &fixture
                .claude_project_dir
                .join("root")
                .join("subagents")
                .join("workflows")
                .join("wf_1")
                .join("agent-b.jsonl"),
            &claude_lines(
                "sess-root",
                true,
                "2026-09-01T00:20:00.000Z",
                "grouped work",
            ),
        );

        let report = build(&fixture, IngestOptions::default());
        assert_eq!(report.sessions_indexed, 3, "issues: {:?}", report.issues);

        let store = open_store(&fixture);
        let sessions = store.list_sessions(false, &[], 10).unwrap();
        let root = sessions
            .iter()
            .find(|session| session.lineage_depth.is_root())
            .expect("root session");
        let children: Vec<_> = sessions
            .iter()
            .filter(|session| !session.lineage_depth.is_root())
            .collect();
        assert_eq!(children.len(), 2);
        for child in &children {
            assert_eq!(
                child.lineage_depth,
                LineageDepth::nested(1),
                "workflows/wf_* is a grouping directory, not a level"
            );
            assert_eq!(
                child.root_session_key, root.session_key,
                "a flattened child must point back at its parent conversation"
            );
        }
    }

    /// AC5's ingest half.
    #[test]
    fn pi_permissions_files_are_not_indexed_as_sessions() {
        let fixture = fixture();
        write(
            &fixture
                .pi_project_dir
                .join("2026-09-03T00-00-00-000Z_aaaaaaaa.jsonl"),
            &pi_lines("pi-1", "2026-09-03T00:00:00.000Z", "real transcript"),
        );
        write(
            &fixture.pi_project_dir.join("permissions.jsonl"),
            &[r#"{"tool":"bash","decision":"allow"}"#.to_string()],
        );

        let report = build(&fixture, IngestOptions::default());
        assert_eq!(report.sessions_indexed, 1);
        assert_eq!(
            report.files_scanned, 1,
            "a non-transcript must not even be opened"
        );
    }

    #[test]
    fn pi_nested_transcripts_carry_their_depth_and_root() {
        let fixture = fixture();
        write(
            &fixture
                .pi_project_dir
                .join("2026-09-01T00-00-00-000Z_aaaaaaaa.jsonl"),
            &pi_lines("pi-root", "2026-09-01T00:00:00.000Z", "root"),
        );
        write(
            &fixture
                .pi_project_dir
                .join("2026-09-01T00-00-00-000Z_aaaaaaaa")
                .join("11111111-2222-3333-4444-555555555555")
                .join("2026-09-01T01-00-00-000Z_bbbbbbbb.jsonl"),
            &pi_lines("pi-child", "2026-09-01T01:00:00.000Z", "child"),
        );

        build(&fixture, IngestOptions::default());
        let store = open_store(&fixture);
        let sessions = store.list_sessions(false, &[], 10).unwrap();
        let child = sessions
            .iter()
            .find(|session| session.vendor_session_id == "pi-child")
            .unwrap();
        let root = sessions
            .iter()
            .find(|session| session.vendor_session_id == "pi-root")
            .unwrap();
        assert_eq!(child.lineage_depth, LineageDepth::nested(1));
        assert_eq!(child.root_session_key, root.session_key);
    }

    /// Incremental builds skip unchanged files and pick up changed ones.
    #[test]
    fn a_second_build_skips_unchanged_transcripts_and_reindexes_changed_ones() {
        let fixture = fixture();
        let path = fixture.claude_project_dir.join("c1.jsonl");
        write(
            &path,
            &claude_lines("sess-c1", false, "2026-09-01T00:00:00.000Z", "original"),
        );

        let first = build(&fixture, IngestOptions::default());
        assert_eq!(first.sessions_indexed, 1);
        assert_eq!(first.sessions_skipped_unchanged, 0);

        let second = build(&fixture, IngestOptions::default());
        assert_eq!(second.sessions_indexed, 0);
        assert_eq!(second.sessions_skipped_unchanged, 1);

        // Change the file; the identity check must notice.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write(
            &path,
            &claude_lines("sess-c1", false, "2026-09-01T00:00:00.000Z", "revised"),
        );
        let third = build(&fixture, IngestOptions::default());
        assert_eq!(third.sessions_indexed, 1);
        assert_eq!(third.sessions_skipped_unchanged, 0);

        let store = open_store(&fixture);
        assert!(store.search("original", false, &[], 10).unwrap().is_empty());
        assert_eq!(store.search("revised", false, &[], 10).unwrap().len(), 1);
    }

    #[test]
    fn a_full_rebuild_reindexes_everything() {
        let fixture = fixture();
        write(
            &fixture.claude_project_dir.join("c1.jsonl"),
            &claude_lines("sess-c1", false, "2026-09-01T00:00:00.000Z", "text"),
        );
        build(&fixture, IngestOptions::default());
        let rebuilt = build(
            &fixture,
            IngestOptions {
                full_rebuild: true,
                ..IngestOptions::default()
            },
        );
        assert_eq!(rebuilt.sessions_indexed, 1);
        assert_eq!(rebuilt.sessions_skipped_unchanged, 0);
    }

    /// A cancelled build must not prune. `seen_keys` only lists what the walk
    /// reached, so pruning against a partial list reads every transcript the
    /// walk never got to as deleted — turning "stop early" into "throw most of
    /// the index away".
    #[test]
    fn a_cancelled_build_keeps_everything_it_has_not_reached_yet() {
        let fixture = fixture();
        let cwd = fixture.project_root.to_string_lossy().into_owned();
        for index in 0..4 {
            write(
                &fixture.claude_project_dir.join(format!("c{index}.jsonl")),
                &claude_lines(
                    &format!("sess-c{index}"),
                    false,
                    "2026-09-01T00:00:00.000Z",
                    &format!("claude {index}"),
                ),
            );
        }
        write(
            &fixture.codex_sessions.join("x1.jsonl"),
            &codex_lines("sess-x1", &cwd, "2026-09-01T00:00:00.000Z", "codex kept"),
        );
        let full = build(&fixture, IngestOptions::default());
        assert_eq!(full.sessions_indexed, 5);
        assert!(!full.cancelled);

        // Cancel before the walk starts: nothing is reached, so nothing may be
        // pruned even though `seen_keys` is empty.
        let cancel = CancelFlag::new();
        cancel.cancel();
        let stopped = build_with_cancel(&fixture, IngestOptions::default(), &cancel);
        assert!(stopped.cancelled, "the report did not say it was cancelled");
        assert_eq!(stopped.files_scanned, 0);
        assert_eq!(
            stopped.sessions_forgotten, 0,
            "a cancelled build pruned {} sessions",
            stopped.sessions_forgotten
        );

        let store = open_store(&fixture);
        assert_eq!(
            store.counts().unwrap().0,
            5,
            "the cancelled build erased sessions it never looked at"
        );
    }

    /// Cancelling partway leaves what was already written and stops the rest.
    #[test]
    fn cancelling_partway_stops_the_walk_and_keeps_what_was_written() {
        let fixture = fixture();
        for index in 0..6 {
            write(
                &fixture.claude_project_dir.join(format!("c{index}.jsonl")),
                &claude_lines(
                    &format!("sess-c{index}"),
                    false,
                    "2026-09-01T00:00:00.000Z",
                    &format!("claude {index}"),
                ),
            );
        }
        let fence = ProjectFence::single(&fixture.project_root).unwrap();
        let cancel = CancelFlag::new();
        let mut seen = 0_u32;
        let report = build_index(
            &fence,
            &fixture.state_root,
            &IngestOptions::default(),
            &cancel,
            &mut |progress: IngestProgress| {
                seen += 1;
                // Pull the plug after the second file.
                if progress.files_seen == 2 {
                    cancel.cancel();
                }
            },
        )
        .unwrap();

        assert!(report.cancelled);
        assert_eq!(seen, 2, "the walk kept going after the cancel");
        assert_eq!(report.files_scanned, 2);
        assert_eq!(report.sessions_forgotten, 0);
        // Whatever was committed is complete and readable.
        assert_eq!(open_store(&fixture).counts().unwrap().0, 2);
    }

    /// Progress carries the project key, so a UI with several projects open can
    /// route the event without inventing a request id.
    #[test]
    fn progress_names_the_project_it_belongs_to() {
        let fixture = fixture();
        write(
            &fixture.claude_project_dir.join("c1.jsonl"),
            &claude_lines("sess-c1", false, "2026-09-01T00:00:00.000Z", "text"),
        );
        let fence = ProjectFence::single(&fixture.project_root).unwrap();
        let expected = fence.namespace_key();
        let mut seen: Vec<IngestProgress> = Vec::new();
        build_index(
            &fence,
            &fixture.state_root,
            &IngestOptions::default(),
            &CancelFlag::new(),
            &mut |progress: IngestProgress| seen.push(progress),
        )
        .unwrap();
        assert!(!seen.is_empty());
        for progress in &seen {
            assert_eq!(progress.project_key, expected);
        }
    }

    /// A skipped-unchanged **sidechain** must not register itself under its
    /// parent's id.
    ///
    /// A sidechain's `vendor_session_id` *is* the parent's id, so the skip
    /// branch filing it into the root map means the next sidechain of that same
    /// conversation resolves its root to a **sibling** instead of falling back
    /// to itself. The `or_insert` and the roots-first sort hide this whenever
    /// the real parent is present — the failure needs a conversation whose root
    /// transcript is no longer in the scanned set, which is what a deleted or
    /// relocated parent looks like on an incremental build.
    ///
    /// The fixture pins the processing order deliberately: `files.sort_by_key`
    /// puts everything outside `subagents/` first, so the top-level sidechain is
    /// always adapted before the nested one.
    #[test]
    fn a_skipped_sidechain_does_not_become_another_sidechains_root() {
        let fixture = fixture();
        // Two sidechains of the same (absent) parent conversation.
        let first_path = fixture.claude_project_dir.join("orphan-a.jsonl");
        let second_path = fixture
            .claude_project_dir
            .join("subagents")
            .join("orphan-b.jsonl");
        write(
            &first_path,
            &claude_lines("sess-parent", true, "2026-09-01T00:00:00.000Z", "branch a"),
        );
        write(
            &second_path,
            &claude_lines("sess-parent", true, "2026-09-01T00:01:00.000Z", "branch b"),
        );

        let first_key = adapters::session_key(MemoryVendor::ClaudeCode, &first_path);
        let second_key = adapters::session_key(MemoryVendor::ClaudeCode, &second_path);

        build(&fixture, IngestOptions::default());
        // With no parent in the scanned set, each is its own root — the honest
        // fallback the adapter documents.
        let store = open_store(&fixture);
        assert_eq!(
            store.get_session(&second_key).unwrap().unwrap().root_session_key,
            second_key
        );
        drop(store);

        // Leave the first untouched (skip branch) and change the second, so the
        // second is re-adapted against whatever the skip branch put in the map.
        write(
            &second_path,
            &claude_lines("sess-parent", true, "2026-09-01T00:02:00.000Z", "branch b revised"),
        );
        let report = build(&fixture, IngestOptions::default());
        assert_eq!(report.sessions_skipped_unchanged, 1);
        assert_eq!(report.sessions_indexed, 1);

        let store = open_store(&fixture);
        let second = store.get_session(&second_key).unwrap().unwrap();
        assert_ne!(
            second.root_session_key, first_key,
            "a sidechain was filed under a sibling sidechain as its root"
        );
        assert_eq!(
            second.root_session_key, second_key,
            "expected the self-root fallback"
        );
    }

    /// A Codex transcript that is momentarily unreadable must not be treated as
    /// deleted. `read_meta` folds a transient I/O failure into the same `None`
    /// as "not a Codex record", and the key used to be recorded only after that
    /// call succeeded — so one unreadable moment during a build erased a session
    /// that was still sitting on disk. Claude and pi never had the problem
    /// because they record the key before reading.
    #[test]
    fn an_unreadable_codex_transcript_is_not_treated_as_deleted() {
        let fixture = fixture();
        let cwd = fixture.project_root.to_string_lossy().into_owned();
        let path = fixture.codex_sessions.join("x1.jsonl");
        write(
            &path,
            &codex_lines("sess-x1", &cwd, "2026-09-01T00:00:00.000Z", "codex kept"),
        );
        let first = build(&fixture, IngestOptions::default());
        assert_eq!(first.sessions_indexed, 1);

        // Still present, but its first line no longer parses as session_meta —
        // the same observable state a truncated write or a permission blip
        // produces.
        std::fs::write(&path, b"not json at all\n").unwrap();

        let second = build(&fixture, IngestOptions::default());
        assert_eq!(
            second.sessions_forgotten, 0,
            "an unreadable but present transcript was pruned"
        );
        let store = open_store(&fixture);
        assert_eq!(
            store.search("kept", true, &[], 10).unwrap().len(),
            1,
            "the previously indexed Codex session was erased"
        );
    }

    /// The other half of the same rule: gone from disk still means forgotten.
    #[test]
    fn a_deleted_codex_transcript_is_still_forgotten() {
        let fixture = fixture();
        let cwd = fixture.project_root.to_string_lossy().into_owned();
        let path = fixture.codex_sessions.join("x1.jsonl");
        write(
            &path,
            &codex_lines("sess-x1", &cwd, "2026-09-01T00:00:00.000Z", "doomed"),
        );
        build(&fixture, IngestOptions::default());
        std::fs::remove_file(&path).unwrap();

        let report = build(&fixture, IngestOptions::default());
        assert_eq!(report.sessions_forgotten, 1);
        assert!(open_store(&fixture)
            .search("doomed", true, &[], 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_deleted_transcript_is_forgotten_on_the_next_build() {
        let fixture = fixture();
        let path = fixture.claude_project_dir.join("c1.jsonl");
        write(
            &path,
            &claude_lines("sess-c1", false, "2026-09-01T00:00:00.000Z", "doomed"),
        );
        build(&fixture, IngestOptions::default());
        std::fs::remove_file(&path).unwrap();

        let report = build(&fixture, IngestOptions::default());
        assert_eq!(report.sessions_forgotten, 1);
        let store = open_store(&fixture);
        assert!(store.search("doomed", true, &[], 10).unwrap().is_empty());
    }

    /// The guard that keeps an unreachable store from reading as a mass
    /// deletion. Without it, a build run while `CODEX_HOME` points elsewhere
    /// would erase every indexed Codex session.
    #[test]
    fn an_unreachable_vendor_store_does_not_erase_its_indexed_sessions() {
        let fixture = fixture();
        write(
            &fixture.claude_project_dir.join("c1.jsonl"),
            &claude_lines("sess-c1", false, "2026-09-01T00:00:00.000Z", "claude kept"),
        );
        write(
            &fixture.codex_sessions.join("rollout-1.jsonl"),
            &codex_lines(
                "codex-1",
                &fixture.project_root.to_string_lossy(),
                "2026-09-01T00:00:00.000Z",
                "codex kept",
            ),
        );
        build(&fixture, IngestOptions::default());

        // The Codex store disappears; Claude's is untouched.
        std::fs::remove_dir_all(fixture.codex_sessions.parent().unwrap().parent().unwrap())
            .unwrap();
        let report = build(&fixture, IngestOptions::default());
        assert_eq!(
            report.sessions_forgotten, 0,
            "an unscanned vendor's sessions must survive"
        );

        let store = open_store(&fixture);
        assert_eq!(store.search("codex kept", false, &[], 10).unwrap().len(), 1);
        assert_eq!(
            store.search("claude kept", false, &[], 10).unwrap().len(),
            1
        );
    }

    #[test]
    fn progress_is_reported_for_every_scanned_file() {
        let fixture = fixture();
        for index in 0..3 {
            write(
                &fixture.claude_project_dir.join(format!("c{index}.jsonl")),
                &claude_lines(
                    &format!("sess-{index}"),
                    false,
                    "2026-09-01T00:00:00.000Z",
                    "text",
                ),
            );
        }
        let fence = ProjectFence::single(&fixture.project_root).unwrap();
        let mut seen = Vec::new();
        build_index(
            &fence,
            &fixture.state_root,
            &IngestOptions::default(),
            &CancelFlag::new(),
            &mut |progress| seen.push(progress),
        )
        .unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[2].files_seen, 3);
        assert_eq!(seen[2].files_total, 3);
        assert_eq!(seen[0].vendor, "claude-code");
    }

    #[test]
    fn an_empty_corpus_produces_an_empty_index_rather_than_an_error() {
        let fixture = fixture();
        let report = build(&fixture, IngestOptions::default());
        assert_eq!(report.sessions_indexed, 0);
        assert_eq!(report.files_scanned, 0);
        assert!(report.issues.is_empty());
        assert!(open_store(&fixture)
            .list_sessions(true, &[], 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_vendor_store_gate_refuses_everything_outside_it() {
        let fixture = fixture();
        let inside = fixture.claude_project_dir.join("c1.jsonl");
        assert!(
            !is_inside_vendor_store(MemoryVendor::ClaudeCode, &inside),
            "a path that does not exist yet cannot be proven safe"
        );
        write(
            &inside,
            &claude_lines("sess-c1", false, "2026-09-01T00:00:00.000Z", "text"),
        );
        assert!(is_inside_vendor_store(MemoryVendor::ClaudeCode, &inside));

        for refused in [Path::new("/etc/passwd"), Path::new("relative.jsonl")] {
            assert!(
                !is_inside_vendor_store(MemoryVendor::ClaudeCode, refused),
                "{} must be refused",
                refused.display()
            );
        }
        // A symlink into the store is still a symlink.
        #[cfg(unix)]
        {
            let link = fixture.claude_project_dir.join("link.jsonl");
            std::os::unix::fs::symlink(&inside, &link).unwrap();
            assert!(!is_inside_vendor_store(MemoryVendor::ClaudeCode, &link));
        }
    }

    /// The ingest spike, run against this machine's real corpus.
    ///
    /// `#[ignore]` because it depends on the operator's own transcripts and can
    /// take minutes; run it with
    /// `cargo test --lib memory_index_spike -- --ignored --nocapture`.
    ///
    /// It exists to close the one number the design analysis could not: index
    /// size and full-build duration were estimated at 3–20 GB and 20–90 minutes
    /// by extrapolating a 15.1% expansion rate from a 75.8 MB sample. Deliberately
    /// in-crate rather than a separate cargo project, so it measures the exact
    /// `rusqlite`/`libsqlite3` build this binary ships rather than recompiling
    /// SQLite from source and reporting a different one.
    #[test]
    #[ignore = "reads the operator's real transcript stores; run explicitly"]
    fn memory_index_spike_over_the_real_corpus() {
        let project_root = std::env::var("MEMORY_INDEX_SPIKE_PROJECT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::current_dir()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .to_path_buf()
            });
        let temp = tempfile::tempdir().unwrap();
        let fence = ProjectFence::single(&project_root).expect("project root");

        let started = Instant::now();
        let report = build_index(
            &fence,
            temp.path(),
            &IngestOptions::default(),
            &CancelFlag::new(),
            &mut |progress: IngestProgress| {
                if progress.files_seen % 50 == 0 {
                    eprintln!(
                        "  {} {}/{}",
                        progress.vendor, progress.files_seen, progress.files_total
                    );
                }
            },
        )
        .expect("build");
        let elapsed = started.elapsed();
        let index_bytes = std::fs::metadata(&report.database_path)
            .map(|meta| meta.len())
            .unwrap_or_default();

        eprintln!("\n=== memory index spike: {} ===", project_root.display());
        eprintln!("files scanned      {}", report.files_scanned);
        eprintln!("sessions indexed   {}", report.sessions_indexed);
        eprintln!("messages indexed   {}", report.messages_indexed);
        eprintln!("compactions        {}", report.compactions_indexed);
        eprintln!("out of scope       {}", report.sessions_out_of_scope);
        eprintln!("issues             {}", report.issues.len());
        for issue in report.issues.iter().take(5) {
            eprintln!("  {} {} {}", issue.code, issue.path, issue.detail);
        }
        eprintln!(
            "index size         {:.2} MB",
            index_bytes as f64 / 1_048_576.0
        );
        eprintln!("duration           {:.1} s", elapsed.as_secs_f64());

        let store = open_store_at(temp.path(), &fence);
        // "The index is about as big as the corpus" is not actionable on its
        // own; what matters is whether the bytes are stored text, the inverted
        // index, or churn. Each of those has a different lever.
        if let Ok((text_bytes, cjk_bytes)) = store.text_vs_cjk_bytes() {
            eprintln!(
                "\ntext column        {:.2} MB\ncjk expansion      {:.2} MB  (+{:.1}% over text)",
                text_bytes as f64 / 1_048_576.0,
                cjk_bytes as f64 / 1_048_576.0,
                cjk_bytes as f64 * 100.0 / text_bytes.max(1) as f64
            );
        }
        let breakdown = store.size_breakdown().unwrap_or_default();
        if !breakdown.is_empty() {
            eprintln!("\nwhere the bytes are:");
            for (name, bytes) in breakdown.iter().take(12) {
                eprintln!(
                    "  {:<28} {:>8.2} MB  {:>5.1}%",
                    name,
                    *bytes as f64 / 1_048_576.0,
                    *bytes as f64 * 100.0 / index_bytes.max(1) as f64
                );
            }
        }
        let sessions = store.list_sessions(false, &[], 10).unwrap();
        eprintln!("\nnewest sessions by FIRST MESSAGE time:");
        for session in sessions.iter().take(10) {
            eprintln!(
                "  {:<12} depth={:?} {} {}",
                session.vendor,
                session.lineage_depth.value(),
                session.first_message_at_utc.as_deref().unwrap_or("-"),
                session.title.as_deref().unwrap_or("")
            );
        }
        let hits = store.search("memory index", false, &[], 5).unwrap();
        eprintln!("\nsample search hits: {}", hits.len());

        assert!(
            report.files_scanned > 0,
            "the spike found no transcripts for {}; set MEMORY_INDEX_SPIKE_PROJECT",
            project_root.display()
        );
    }

    fn open_store_at(state_root: &Path, fence: &ProjectFence) -> MemoryStore {
        let location = fence.index_location(state_root).unwrap();
        MemoryStore::open(&location.database_path, &location.namespace_key).unwrap()
    }

    #[test]
    fn codex_root_resolution_walks_the_spawn_chain_and_survives_a_cycle() {
        let mut threads = HashMap::new();
        threads.insert("root".to_string(), ("codex:/root.jsonl".to_string(), None));
        threads.insert(
            "mid".to_string(),
            ("codex:/mid.jsonl".to_string(), Some("root".to_string())),
        );
        threads.insert(
            "leaf".to_string(),
            ("codex:/leaf.jsonl".to_string(), Some("mid".to_string())),
        );
        assert_eq!(
            resolve_codex_root(&threads, "leaf").as_deref(),
            Some("codex:/root.jsonl")
        );
        assert_eq!(resolve_codex_root(&threads, "missing"), None);

        let mut cyclic = HashMap::new();
        cyclic.insert(
            "a".to_string(),
            ("codex:/a".to_string(), Some("b".to_string())),
        );
        cyclic.insert(
            "b".to_string(),
            ("codex:/b".to_string(), Some("a".to_string())),
        );
        assert!(
            resolve_codex_root(&cyclic, "a").is_some(),
            "a cycle in vendor data must terminate, not hang"
        );
    }
}
