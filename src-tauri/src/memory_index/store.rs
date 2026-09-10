//! SQLite + FTS5 storage for one project's index.
//!
//! Engine choice and its rejected alternative are settled upstream: JSON/JSONL
//! as the primary index has no full-text search, no incremental upsert, needs a
//! hand-rolled journal for interrupted writes, and would amount to inventing a
//! seventh transcript format. `rusqlite 0.32 + bundled` is already in the
//! dependency tree and `libsqlite3-sys`'s bundled build passes
//! `-DSQLITE_ENABLE_FTS5` unconditionally, so full-text search costs no new
//! dependency.
//!
//! ## Where the text lives
//!
//! Exactly once, in the FTS5 table. `messages` holds metadata plus an
//! [`crate::memory_index::types::SourcePointer`] and the FTS rowid; the
//! searchable text is a column of `messages_fts`. Storing it in both would
//! double the largest thing in the index for no gain, and the upstream decision
//! is explicit that the index keeps searchable text plus a pointer rather than a
//! copy of the corpus.
//!
//! The one exception is the `cjk` column, which holds a bigram expansion of the
//! text's CJK runs and nothing else — see [`super::cjk`] for why `unicode61`
//! alone cannot find a Chinese word inside a sentence. Latin-only records expand
//! to the empty string, so the exception costs nothing on the majority of the
//! corpus.
//!
//! ## Re-ingest
//!
//! Per session, delete-then-insert inside one transaction. That is what makes a
//! repeated build idempotent without needing row-level diffing, and it is why
//! `messages.fts_rowid` exists: an FTS5 row can only be deleted by rowid.

use std::path::Path;

use rusqlite::types::ToSql;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Transaction};

use super::cjk;
use super::paths::MemoryVendor;
use super::types::{
    CompactionRecord, IndexedSession, LineageDepth, NormalizedMessage, NormalizedRole,
    SessionScope, SourcePointer, TimestampConfidence, SCHEMA_VERSION,
};
use super::{MemoryIndexError, MemoryIndexResult, ERR_STORE_FAILED};

const META_SCHEMA_VERSION: &str = "schema_version";
const META_STORE_VERSION: &str = "store_version";
const META_PROJECT_KEY: &str = "project_key";

/// Version of the **on-disk table layout**, separate from
/// [`SCHEMA_VERSION`], which versions the normalized record shape on the wire.
///
/// They were one number and had to be split: adding the `cjk` column changed how
/// the database is laid out without changing a single field of
/// [`NormalizedMessage`], and bumping the record version for that would have
/// told every client its contract had changed when it had not.
///
/// * 1 — single-column `messages_fts`.
/// * 2 — `messages_fts(text, cjk)`.
const STORE_VERSION: u32 = 2;

/// Hard cap on rows returned by any single query, applied after the caller's
/// own limit. An MCP client asking for everything would otherwise be able to
/// pull a multi-gigabyte corpus through one tool call.
pub const MAX_QUERY_LIMIT: usize = 200;

fn store_error(operation: &'static str) -> impl Fn(rusqlite::Error) -> MemoryIndexError {
    move |error| MemoryIndexError::new(ERR_STORE_FAILED, format!("{operation}: {error}"))
}

/// One project's index database.
#[derive(Debug)]
pub struct MemoryStore {
    connection: Connection,
    project_key: String,
}

/// A single search result, flat because all three transports return it as-is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchHit {
    pub message_key: String,
    pub session_key: String,
    pub root_session_key: String,
    pub vendor: String,
    pub lineage_depth: LineageDepth,
    pub ordinal: u32,
    pub role: NormalizedRole,
    pub timestamp_utc: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub timestamp_confidence: TimestampConfidence,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub text: String,
    pub session_title: Option<String>,
    pub session_first_message_at_utc: Option<String>,
    pub session_scope: SessionScope,
    pub source: SourcePointer,
    /// Whether the recorded byte range still hashes to what was indexed.
    /// The store never checks it (that would mean an I/O per row); the service
    /// layer verifies before returning, which is where the authorization and
    /// freshness re-checks belong.
    pub source_fresh: bool,
}

/// What one build pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreWriteReport {
    pub sessions_written: u32,
    pub messages_written: u32,
    pub compactions_written: u32,
}

impl MemoryStore {
    /// Open (creating if needed) the index for one project.
    ///
    /// The FTS5 probe is deliberate. The bundled build enables FTS5
    /// unconditionally today, but `rusqlite` is declared without an `fts5`
    /// feature, so nothing in the manifest states the requirement. A probe turns
    /// "every query silently fails" into one clear error at open time.
    pub fn open(database_path: &Path, project_key: &str) -> MemoryIndexResult<Self> {
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                MemoryIndexError::new(
                    ERR_STORE_FAILED,
                    format!("create index dir {}: {error}", parent.display()),
                )
            })?;
        }
        let connection = Connection::open(database_path).map_err(store_error("open database"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 PRAGMA foreign_keys=ON;
                 PRAGMA busy_timeout=5000;",
            )
            .map_err(store_error("configure database"))?;
        probe_fts5(&connection)?;

        let store = Self {
            connection,
            project_key: project_key.to_string(),
        };
        store.migrate()?;
        store.check_project_key()?;
        Ok(store)
    }

    /// An in-memory index. Used by tests, and by the read-only MCP surface when
    /// a project has no index yet — an empty result is a better answer than a
    /// spuriously created database file.
    pub fn open_in_memory(project_key: &str) -> MemoryIndexResult<Self> {
        let connection = Connection::open_in_memory().map_err(store_error("open memory db"))?;
        probe_fts5(&connection)?;
        let store = Self {
            connection,
            project_key: project_key.to_string(),
        };
        store.migrate()?;
        store.check_project_key()?;
        Ok(store)
    }

    #[must_use]
    pub fn project_key(&self) -> &str {
        &self.project_key
    }

    /// Bring the database up to [`STORE_VERSION`], discarding its contents when
    /// the layout changed.
    ///
    /// Dropping is the correct migration here and not a shortcut: this database
    /// is a derived cache of transcripts that are still on disk, so the entire
    /// cost of throwing it away is one rebuild, while hand-migrating an FTS5
    /// table would mean reading and re-tokenizing every row anyway. `meta`
    /// survives the drop so the project binding written by
    /// [`Self::check_project_key`] is not silently re-established against a
    /// different project.
    fn migrate(&self) -> MemoryIndexResult<()> {
        self.connection
            .execute_batch(META_SQL)
            .map_err(store_error("apply meta schema"))?;
        let stored: Option<u32> = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_STORE_VERSION],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(store_error("read store version"))?
            .and_then(|raw| raw.parse().ok());
        // `None` covers both a fresh database and a version-1 one, which never
        // wrote this key. Dropping tables that do not exist is a no-op.
        if stored != Some(STORE_VERSION) {
            self.connection
                .execute_batch(DROP_SQL)
                .map_err(store_error("drop outdated schema"))?;
        }
        self.connection
            .execute_batch(SCHEMA_SQL)
            .map_err(store_error("apply schema"))?;
        for (key, value) in [
            (META_SCHEMA_VERSION, SCHEMA_VERSION.to_string()),
            (META_STORE_VERSION, STORE_VERSION.to_string()),
        ] {
            self.connection
                .execute(
                    "INSERT INTO meta(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )
                .map_err(store_error("record schema version"))?;
        }
        Ok(())
    }

    /// Bind the database to its project on first open, and refuse to serve it
    /// under a different one afterwards.
    ///
    /// The namespace directory already separates projects, so this is the
    /// second lock on the same door: if a database is ever moved, copied or
    /// symlinked into another project's namespace, it stops answering rather
    /// than answering with the wrong project's history.
    fn check_project_key(&self) -> MemoryIndexResult<()> {
        let existing: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_PROJECT_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(store_error("read project key"))?;
        match existing {
            None => {
                self.connection
                    .execute(
                        "INSERT INTO meta(key, value) VALUES (?1, ?2)",
                        params![META_PROJECT_KEY, &self.project_key],
                    )
                    .map_err(store_error("record project key"))?;
                Ok(())
            }
            Some(stored) if stored == self.project_key => Ok(()),
            Some(stored) => Err(MemoryIndexError::new(
                ERR_STORE_FAILED,
                format!(
                    "index belongs to project {stored}, refusing to open it as {}",
                    self.project_key
                ),
            )),
        }
    }

    /// The file identity recorded for a session, if it is already indexed.
    /// Ingest uses this to skip unchanged transcripts.
    pub fn indexed_file_identity(
        &self,
        session_key: &str,
    ) -> MemoryIndexResult<Option<super::types::FileIdentity>> {
        self.connection
            .query_row(
                "SELECT src_device, src_inode, src_size, src_mtime_ms
                 FROM sessions WHERE session_key = ?1",
                params![session_key],
                |row| {
                    Ok(super::types::FileIdentity {
                        device: row.get::<_, i64>(0)? as u64,
                        inode: row.get::<_, i64>(1)? as u64,
                        size_bytes: row.get::<_, i64>(2)? as u64,
                        modified_unix_ms: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(store_error("read indexed identity"))
    }

    /// Replace everything stored for one session, in a single transaction.
    pub fn replace_session(
        &mut self,
        session: &IndexedSession,
        messages: &[NormalizedMessage],
        compactions: &[CompactionRecord],
    ) -> MemoryIndexResult<StoreWriteReport> {
        let transaction = self
            .connection
            .transaction()
            .map_err(store_error("begin transaction"))?;
        delete_session_rows(&transaction, &session.session_key)?;
        write_session_row(&transaction, session)?;
        for message in messages {
            write_message_row(&transaction, message)?;
        }
        for compaction in compactions {
            write_compaction_row(&transaction, compaction)?;
        }
        transaction
            .commit()
            .map_err(store_error("commit transaction"))?;
        Ok(StoreWriteReport {
            sessions_written: 1,
            messages_written: messages.len() as u32,
            compactions_written: compactions.len() as u32,
        })
    }

    /// Drop sessions that are in the index but no longer on disk.
    pub fn forget_sessions(&mut self, session_keys: &[String]) -> MemoryIndexResult<u32> {
        if session_keys.is_empty() {
            return Ok(0);
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(store_error("begin forget"))?;
        for key in session_keys {
            delete_session_rows(&transaction, key)?;
        }
        transaction.commit().map_err(store_error("commit forget"))?;
        Ok(session_keys.len() as u32)
    }

    /// Every session key currently in the index.
    pub fn all_session_keys(&self) -> MemoryIndexResult<Vec<String>> {
        let mut statement = self
            .connection
            .prepare("SELECT session_key FROM sessions")
            .map_err(store_error("prepare session keys"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(store_error("query session keys"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(store_error("read session keys"))
    }

    /// Sessions ordered by **first message time**, newest first.
    ///
    /// This is the ordering the whole feature was asked for. The existing
    /// discovery path sorts by file mtime, which reorders 9 of this project's 14
    /// Claude sessions and drifts by up to six days. `NULL` first-message times
    /// sort last rather than first: a session whose time is unknown should not
    /// outrank every session whose time is known.
    pub fn list_sessions(
        &self,
        include_unscoped: bool,
        vendors: &[MemoryVendor],
        limit: usize,
    ) -> MemoryIndexResult<Vec<IndexedSession>> {
        let limit = clamp_limit(limit);
        let (vendor_sql, vendor_values) = vendor_filter(vendors);
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {SESSION_COLUMNS} FROM sessions s
                 WHERE (? OR s.scope = 'scoped'){vendor_sql}
                 ORDER BY s.first_message_at_ms IS NULL, s.first_message_at_ms DESC,
                          s.session_key ASC
                 LIMIT ?"
            ))
            .map_err(store_error("prepare list sessions"))?;
        let mut bound: Vec<Box<dyn ToSql>> = vec![Box::new(include_unscoped)];
        bound.extend(
            vendor_values
                .into_iter()
                .map(|value| Box::new(value) as Box<dyn ToSql>),
        );
        bound.push(Box::new(limit as i64));
        let rows = statement
            .query_map(params_from_iter(bound.iter()), read_session_row)
            .map_err(store_error("query sessions"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(store_error("read sessions"))
    }

    pub fn get_session(&self, session_key: &str) -> MemoryIndexResult<Option<IndexedSession>> {
        self.connection
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE session_key = ?1"),
                params![session_key],
                read_session_row,
            )
            .optional()
            .map_err(store_error("get session"))
    }

    /// One session's messages in transcript order.
    pub fn session_messages(
        &self,
        session_key: &str,
        limit: usize,
    ) -> MemoryIndexResult<Vec<MemorySearchHit>> {
        let limit = clamp_limit(limit);
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {HIT_COLUMNS}
                 FROM messages m
                 JOIN sessions s ON s.session_key = m.session_key
                 JOIN messages_fts f ON f.rowid = m.fts_rowid
                 WHERE m.session_key = ?1
                 ORDER BY m.ordinal ASC
                 LIMIT ?2"
            ))
            .map_err(store_error("prepare session messages"))?;
        let rows = statement
            .query_map(params![session_key, limit as i64], read_hit_row)
            .map_err(store_error("query session messages"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(store_error("read session messages"))
    }

    /// Full-text search over normalized message text.
    ///
    /// The caller's query is treated as literal terms, not FTS5 syntax — see
    /// [`to_fts_match`] for why.
    pub fn search(
        &self,
        query: &str,
        include_unscoped: bool,
        vendors: &[MemoryVendor],
        limit: usize,
    ) -> MemoryIndexResult<Vec<MemorySearchHit>> {
        let limit = clamp_limit(limit);
        let Some(match_expression) = to_fts_match(query) else {
            return Ok(Vec::new());
        };
        let (vendor_sql, vendor_values) = vendor_filter(vendors);
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {HIT_COLUMNS}
                 FROM messages_fts f
                 JOIN messages m ON m.fts_rowid = f.rowid
                 JOIN sessions s ON s.session_key = m.session_key
                 WHERE messages_fts MATCH ? AND (? OR s.scope = 'scoped'){vendor_sql}
                 ORDER BY bm25(messages_fts) ASC, m.timestamp_ms DESC
                 LIMIT ?"
            ))
            .map_err(store_error("prepare search"))?;
        let mut bound: Vec<Box<dyn ToSql>> =
            vec![Box::new(match_expression), Box::new(include_unscoped)];
        bound.extend(
            vendor_values
                .into_iter()
                .map(|value| Box::new(value) as Box<dyn ToSql>),
        );
        bound.push(Box::new(limit as i64));
        let rows = statement
            .query_map(params_from_iter(bound.iter()), read_hit_row)
            .map_err(store_error("query search"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(store_error("read search"))
    }

    /// Compaction summaries matching a query, newest first.
    ///
    /// Surfaced ahead of ordinary matches by the service layer. These are the
    /// only agent-authored summaries in any of the three corpora, and each names
    /// the entries that were dropped from the active context — which is the
    /// concrete thing "memory bank" can actually deliver.
    pub fn search_compactions(
        &self,
        query: &str,
        include_unscoped: bool,
        limit: usize,
    ) -> MemoryIndexResult<Vec<CompactionRecord>> {
        let limit = clamp_limit(limit);
        let like = match query.trim() {
            "" => "%".to_string(),
            trimmed => format!(
                "%{}%",
                trimmed
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            ),
        };
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.session_key, c.root_session_key, c.ordinal, c.summary, c.tokens_before,
                        c.first_kept_entry_id, c.timestamp_utc, c.timestamp_ms,
                        c.src_path, c.src_device, c.src_inode, c.src_size, c.src_mtime_ms,
                        c.src_hash, c.src_offset, c.src_len
                 FROM compactions c
                 JOIN sessions s ON s.session_key = c.session_key
                 WHERE c.summary LIKE ?1 ESCAPE '\\' AND (?2 OR s.scope = 'scoped')
                 ORDER BY c.timestamp_ms IS NULL, c.timestamp_ms DESC
                 LIMIT ?3",
            )
            .map_err(store_error("prepare compaction search"))?;
        let rows = statement
            .query_map(params![like, include_unscoped, limit as i64], |row| {
                Ok(CompactionRecord {
                    schema_version: SCHEMA_VERSION,
                    session_key: row.get(0)?,
                    root_session_key: row.get(1)?,
                    ordinal: row.get::<_, i64>(2)? as u32,
                    summary: row.get(3)?,
                    tokens_before: row.get::<_, Option<i64>>(4)?.map(|value| value as u64),
                    first_kept_entry_id: row.get(5)?,
                    timestamp_utc: row.get(6)?,
                    timestamp_ms: row.get(7)?,
                    source: read_pointer(row, 8)?,
                })
            })
            .map_err(store_error("query compaction search"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(store_error("read compaction search"))
    }

    /// Row counts, for the build report and for tests.
    pub fn counts(&self) -> MemoryIndexResult<(u64, u64, u64)> {
        let sessions = self
            .connection
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(store_error("count sessions"))?;
        let messages = self
            .connection
            .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(store_error("count messages"))?;
        let compactions = self
            .connection
            .query_row("SELECT COUNT(*) FROM compactions", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(store_error("count compactions"))?;
        Ok((sessions as u64, messages as u64, compactions as u64))
    }
}

/// SQL fragment and bound values for an optional vendor filter.
///
/// An empty vendor list means **all three**, not none: memory is looked up
/// across agents by default. Which CLI happened to be running when something
/// was worked out is rarely what you remember about it, so narrowing to one
/// agent is an opt-in refinement rather than the shape of the question.
fn vendor_filter(vendors: &[MemoryVendor]) -> (String, Vec<String>) {
    if vendors.is_empty() {
        return (String::new(), Vec::new());
    }
    let placeholders = vec!["?"; vendors.len()].join(", ");
    (
        format!(" AND s.vendor IN ({placeholders})"),
        vendors
            .iter()
            .map(|vendor| vendor.as_str().to_string())
            .collect(),
    )
}

fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_QUERY_LIMIT)
}

fn probe_fts5(connection: &Connection) -> MemoryIndexResult<()> {
    connection
        .execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS temp.memory_index_fts5_probe USING fts5(probe);
             DROP TABLE temp.memory_index_fts5_probe;",
        )
        .map_err(|error| {
            MemoryIndexError::new(
                ERR_STORE_FAILED,
                format!(
                    "this SQLite build has no FTS5 module, so the memory index cannot be \
                     searched: {error}"
                ),
            )
        })
}

/// Turn a user query into an FTS5 `MATCH` expression made only of quoted,
/// column-qualified terms.
///
/// v1 does literal term matching with implicit AND, not FTS5 query syntax. The
/// index is full of tool output — file paths, flags, error codes — where `-`,
/// `*`, `:` and `"` are ordinary characters, and passing them through would turn
/// a search for `--no-verify` into a syntax error or a `NOT` operator. Quoting
/// every term makes the whole space of user input safe and predictable, at the
/// cost of not offering operators.
///
/// Each whitespace-separated term is then split at the CJK boundary, because the
/// two halves are searched in different columns: Latin runs against `text`,
/// which `unicode61` tokenizes correctly, and CJK runs against the bigram
/// expansion in `cjk`, which is the only way a two-character Chinese word can be
/// found inside a sentence. A mixed term like `ENOENT错误` therefore produces one
/// term of each.
///
/// Returns `None` for a query with no usable terms; the caller returns no hits
/// rather than matching everything.
#[must_use]
pub fn to_fts_match(query: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in query.split_whitespace() {
        for run in split_at_cjk_boundary(word) {
            match run {
                QueryRun::Cjk(text) => terms.extend(
                    cjk::query_terms(&text)
                        .into_iter()
                        .map(|term| format!("{{cjk}}: {term}")),
                ),
                // A run of pure punctuation carries no token, and an FTS5 phrase
                // with no tokens matches nothing useful — dropping it keeps a
                // query like `foo :: bar` meaning `foo AND bar`.
                QueryRun::Other(text) if text.chars().any(char::is_alphanumeric) => terms
                    .push(format!("{{text}}: \"{}\"", text.replace('"', "\"\""))),
                QueryRun::Other(_) => {}
            }
        }
    }
    if terms.is_empty() {
        return None;
    }
    Some(terms.join(" "))
}

enum QueryRun {
    Cjk(String),
    Other(String),
}

fn split_at_cjk_boundary(word: &str) -> Vec<QueryRun> {
    let mut runs: Vec<QueryRun> = Vec::new();
    let mut current = String::new();
    let mut current_is_cjk = false;

    for character in word.chars() {
        let is_cjk = cjk::is_cjk(character);
        if !current.is_empty() && is_cjk != current_is_cjk {
            runs.push(finish_run(std::mem::take(&mut current), current_is_cjk));
        }
        current_is_cjk = is_cjk;
        current.push(character);
    }
    if !current.is_empty() {
        runs.push(finish_run(current, current_is_cjk));
    }
    runs
}

fn finish_run(text: String, is_cjk: bool) -> QueryRun {
    if is_cjk {
        QueryRun::Cjk(text)
    } else {
        QueryRun::Other(text)
    }
}

/// FTS5 rows can only be removed by rowid, which is why `messages.fts_rowid` is
/// stored. Deleting the metadata row first would lose the only handle to the
/// text.
fn delete_session_rows(transaction: &Transaction<'_>, session_key: &str) -> MemoryIndexResult<()> {
    transaction
        .execute(
            "DELETE FROM messages_fts WHERE rowid IN
                 (SELECT fts_rowid FROM messages WHERE session_key = ?1)",
            params![session_key],
        )
        .map_err(store_error("delete fts rows"))?;
    transaction
        .execute(
            "DELETE FROM messages WHERE session_key = ?1",
            params![session_key],
        )
        .map_err(store_error("delete messages"))?;
    transaction
        .execute(
            "DELETE FROM compactions WHERE session_key = ?1",
            params![session_key],
        )
        .map_err(store_error("delete compactions"))?;
    transaction
        .execute(
            "DELETE FROM sessions WHERE session_key = ?1",
            params![session_key],
        )
        .map_err(store_error("delete session"))?;
    Ok(())
}

fn write_session_row(
    transaction: &Transaction<'_>,
    session: &IndexedSession,
) -> MemoryIndexResult<()> {
    transaction
        .execute(
            "INSERT INTO sessions(
                session_key, vendor, vendor_session_id, root_session_key, lineage_depth,
                project_key, scope, cwd, title,
                first_message_at_utc, first_message_at_ms,
                last_activity_at_utc, last_activity_at_ms,
                timestamp_confidence, message_count, tool_count, file_path,
                src_path, src_device, src_inode, src_size, src_mtime_ms,
                src_hash, src_offset, src_len
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9,
                ?10, ?11,
                ?12, ?13,
                ?14, ?15, ?16, ?17,
                ?18, ?19, ?20, ?21, ?22,
                ?23, ?24, ?25
             )",
            params![
                session.session_key,
                session.vendor,
                session.vendor_session_id,
                session.root_session_key,
                session.lineage_depth.value().map(i64::from),
                session.project_key,
                scope_text(session.scope),
                session.cwd,
                session.title,
                session.first_message_at_utc,
                session.first_message_at_ms,
                session.last_activity_at_utc,
                session.last_activity_at_ms,
                session.timestamp_confidence.as_str(),
                session.message_count as i64,
                session.tool_count as i64,
                session.file_path,
                session.source.file_path,
                session.source.device as i64,
                session.source.inode as i64,
                session.source.size_bytes as i64,
                session.source.modified_unix_ms,
                session.source.content_hash,
                session.source.byte_offset as i64,
                session.source.byte_len as i64,
            ],
        )
        .map_err(store_error("insert session"))?;
    Ok(())
}

fn write_message_row(
    transaction: &Transaction<'_>,
    message: &NormalizedMessage,
) -> MemoryIndexResult<()> {
    transaction
        .execute(
            "INSERT INTO messages_fts(text, cjk) VALUES (?1, ?2)",
            params![message.text, cjk::expand(&message.text)],
        )
        .map_err(store_error("insert fts text"))?;
    let fts_rowid = transaction.last_insert_rowid();
    transaction
        .execute(
            "INSERT INTO messages(
                message_key, session_key, root_session_key, lineage_depth, ordinal, role,
                timestamp_utc, timestamp_ms, timestamp_confidence,
                tool_name, tool_call_id, fts_rowid,
                src_path, src_device, src_inode, src_size, src_mtime_ms,
                src_hash, src_offset, src_len
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9,
                ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17,
                ?18, ?19, ?20
             )",
            params![
                message.message_key,
                message.session_key,
                message.root_session_key,
                message.lineage_depth.value().map(i64::from),
                message.ordinal as i64,
                message.role.as_str(),
                message.timestamp_utc,
                message.timestamp_ms,
                message.timestamp_confidence.as_str(),
                message.tool_name,
                message.tool_call_id,
                fts_rowid,
                message.source.file_path,
                message.source.device as i64,
                message.source.inode as i64,
                message.source.size_bytes as i64,
                message.source.modified_unix_ms,
                message.source.content_hash,
                message.source.byte_offset as i64,
                message.source.byte_len as i64,
            ],
        )
        .map_err(store_error("insert message"))?;
    Ok(())
}

fn write_compaction_row(
    transaction: &Transaction<'_>,
    record: &CompactionRecord,
) -> MemoryIndexResult<()> {
    transaction
        .execute(
            "INSERT INTO compactions(
                session_key, root_session_key, ordinal, summary, tokens_before,
                first_kept_entry_id, timestamp_utc, timestamp_ms,
                src_path, src_device, src_inode, src_size, src_mtime_ms,
                src_hash, src_offset, src_len
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                record.session_key,
                record.root_session_key,
                record.ordinal as i64,
                record.summary,
                record.tokens_before.map(|value| value as i64),
                record.first_kept_entry_id,
                record.timestamp_utc,
                record.timestamp_ms,
                record.source.file_path,
                record.source.device as i64,
                record.source.inode as i64,
                record.source.size_bytes as i64,
                record.source.modified_unix_ms,
                record.source.content_hash,
                record.source.byte_offset as i64,
                record.source.byte_len as i64,
            ],
        )
        .map_err(store_error("insert compaction"))?;
    Ok(())
}

const SESSION_COLUMNS: &str = "session_key, vendor, vendor_session_id, root_session_key,
     lineage_depth, project_key, scope, cwd, title,
     first_message_at_utc, first_message_at_ms, last_activity_at_utc, last_activity_at_ms,
     timestamp_confidence, message_count, tool_count, file_path,
     src_path, src_device, src_inode, src_size, src_mtime_ms, src_hash, src_offset, src_len";

const HIT_COLUMNS: &str = "m.message_key, m.session_key, m.root_session_key, s.vendor,
     m.lineage_depth, m.ordinal, m.role, m.timestamp_utc, m.timestamp_ms,
     m.timestamp_confidence, m.tool_name, m.tool_call_id, f.text,
     s.title, s.first_message_at_utc, s.scope,
     m.src_path, m.src_device, m.src_inode, m.src_size, m.src_mtime_ms,
     m.src_hash, m.src_offset, m.src_len";

fn read_pointer(row: &rusqlite::Row<'_>, base: usize) -> rusqlite::Result<SourcePointer> {
    Ok(SourcePointer {
        file_path: row.get(base)?,
        device: row.get::<_, i64>(base + 1)? as u64,
        inode: row.get::<_, i64>(base + 2)? as u64,
        size_bytes: row.get::<_, i64>(base + 3)? as u64,
        modified_unix_ms: row.get(base + 4)?,
        content_hash: row.get(base + 5)?,
        byte_offset: row.get::<_, i64>(base + 6)? as u64,
        byte_len: row.get::<_, i64>(base + 7)? as u64,
    })
}

fn read_lineage(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<LineageDepth> {
    Ok(LineageDepth::from_stored(
        row.get::<_, Option<i64>>(index)?
            .and_then(|value| u8::try_from(value).ok()),
    ))
}

fn read_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexedSession> {
    Ok(IndexedSession {
        schema_version: SCHEMA_VERSION,
        session_key: row.get(0)?,
        vendor: row.get(1)?,
        vendor_session_id: row.get(2)?,
        root_session_key: row.get(3)?,
        lineage_depth: read_lineage(row, 4)?,
        project_key: row.get(5)?,
        scope: parse_scope(&row.get::<_, String>(6)?),
        cwd: row.get(7)?,
        title: row.get(8)?,
        first_message_at_utc: row.get(9)?,
        first_message_at_ms: row.get(10)?,
        last_activity_at_utc: row.get(11)?,
        last_activity_at_ms: row.get(12)?,
        timestamp_confidence: TimestampConfidence::parse(&row.get::<_, String>(13)?),
        message_count: row.get::<_, i64>(14)? as u64,
        tool_count: row.get::<_, i64>(15)? as u64,
        file_path: row.get(16)?,
        source: read_pointer(row, 17)?,
    })
}

fn read_hit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemorySearchHit> {
    Ok(MemorySearchHit {
        message_key: row.get(0)?,
        session_key: row.get(1)?,
        root_session_key: row.get(2)?,
        vendor: row.get(3)?,
        lineage_depth: read_lineage(row, 4)?,
        ordinal: row.get::<_, i64>(5)? as u32,
        role: NormalizedRole::parse(&row.get::<_, String>(6)?).unwrap_or(NormalizedRole::System),
        timestamp_utc: row.get(7)?,
        timestamp_ms: row.get(8)?,
        timestamp_confidence: TimestampConfidence::parse(&row.get::<_, String>(9)?),
        tool_name: row.get(10)?,
        tool_call_id: row.get(11)?,
        text: row.get(12)?,
        session_title: row.get(13)?,
        session_first_message_at_utc: row.get(14)?,
        session_scope: parse_scope(&row.get::<_, String>(15)?),
        source: read_pointer(row, 16)?,
        source_fresh: true,
    })
}

fn scope_text(scope: SessionScope) -> &'static str {
    match scope {
        SessionScope::Scoped => "scoped",
        SessionScope::Unscoped => "unscoped",
    }
}

/// Anything that is not exactly `scoped` is treated as unproven. Failing closed
/// here means a corrupt or future value excludes a session from results rather
/// than admitting it into a project it may not belong to.
fn parse_scope(raw: &str) -> SessionScope {
    if raw == "scoped" {
        SessionScope::Scoped
    } else {
        SessionScope::Unscoped
    }
}

/// Created before the version check, because the version lives in it.
const META_SQL: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Children before parents: `messages` and `compactions` reference `sessions`,
/// and `PRAGMA foreign_keys=ON` is set at open.
const DROP_SQL: &str = "
DROP TABLE IF EXISTS messages_fts;
DROP TABLE IF EXISTS messages;
DROP TABLE IF EXISTS compactions;
DROP TABLE IF EXISTS sessions;
";

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    session_key          TEXT PRIMARY KEY,
    vendor               TEXT NOT NULL,
    vendor_session_id    TEXT NOT NULL,
    root_session_key     TEXT NOT NULL,
    -- NULL means the vendor records no depth. Never defaulted to 0.
    lineage_depth        INTEGER,
    project_key          TEXT NOT NULL,
    scope                TEXT NOT NULL,
    cwd                  TEXT,
    title                TEXT,
    first_message_at_utc TEXT,
    first_message_at_ms  INTEGER,
    last_activity_at_utc TEXT,
    last_activity_at_ms  INTEGER,
    timestamp_confidence TEXT NOT NULL,
    message_count        INTEGER NOT NULL,
    tool_count           INTEGER NOT NULL,
    file_path            TEXT NOT NULL,
    src_path             TEXT NOT NULL,
    src_device           INTEGER NOT NULL,
    src_inode            INTEGER NOT NULL,
    src_size             INTEGER NOT NULL,
    src_mtime_ms         INTEGER NOT NULL,
    src_hash             TEXT NOT NULL,
    src_offset           INTEGER NOT NULL,
    src_len              INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_first_message ON sessions(first_message_at_ms);
CREATE INDEX IF NOT EXISTS sessions_root ON sessions(root_session_key);
CREATE INDEX IF NOT EXISTS sessions_scope ON sessions(scope);

CREATE TABLE IF NOT EXISTS messages (
    message_key          TEXT PRIMARY KEY,
    session_key          TEXT NOT NULL REFERENCES sessions(session_key) ON DELETE CASCADE,
    root_session_key     TEXT NOT NULL,
    lineage_depth        INTEGER,
    ordinal              INTEGER NOT NULL,
    role                 TEXT NOT NULL,
    timestamp_utc        TEXT,
    timestamp_ms         INTEGER,
    timestamp_confidence TEXT NOT NULL,
    tool_name            TEXT,
    tool_call_id         TEXT,
    -- The only handle to this message's text, which lives in messages_fts.
    fts_rowid            INTEGER NOT NULL,
    src_path             TEXT NOT NULL,
    src_device           INTEGER NOT NULL,
    src_inode            INTEGER NOT NULL,
    src_size             INTEGER NOT NULL,
    src_mtime_ms         INTEGER NOT NULL,
    src_hash             TEXT NOT NULL,
    src_offset           INTEGER NOT NULL,
    src_len              INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_session ON messages(session_key, ordinal);
CREATE UNIQUE INDEX IF NOT EXISTS messages_fts_rowid ON messages(fts_rowid);
CREATE INDEX IF NOT EXISTS messages_tool_call ON messages(tool_call_id);

CREATE TABLE IF NOT EXISTS compactions (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key          TEXT NOT NULL REFERENCES sessions(session_key) ON DELETE CASCADE,
    root_session_key     TEXT NOT NULL,
    ordinal              INTEGER NOT NULL,
    summary              TEXT NOT NULL,
    tokens_before        INTEGER,
    first_kept_entry_id  TEXT,
    timestamp_utc        TEXT,
    timestamp_ms         INTEGER,
    src_path             TEXT NOT NULL,
    src_device           INTEGER NOT NULL,
    src_inode            INTEGER NOT NULL,
    src_size             INTEGER NOT NULL,
    src_mtime_ms         INTEGER NOT NULL,
    src_hash             TEXT NOT NULL,
    src_offset           INTEGER NOT NULL,
    src_len              INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS compactions_session ON compactions(session_key);

-- `text` is the exact text a hit displays and the Latin search surface.
-- `cjk` holds only the bigram expansion of the text's CJK runs; it is empty for
-- Latin-only records. See `super::cjk` for why `unicode61` alone cannot find a
-- Chinese word inside a sentence.
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(text, cjk, tokenize = 'unicode61');
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_index::types::FileIdentity;

    const PROJECT: &str = "termul-0011223344556677";

    fn identity() -> FileIdentity {
        FileIdentity {
            device: 1,
            inode: 2,
            size_bytes: 3,
            modified_unix_ms: 4,
        }
    }

    fn pointer(path: &str, offset: u64, bytes: &[u8]) -> SourcePointer {
        SourcePointer::for_record(&identity(), path, offset, bytes)
    }

    fn session(key: &str, first_ms: Option<i64>, scope: SessionScope) -> IndexedSession {
        IndexedSession {
            schema_version: SCHEMA_VERSION,
            session_key: key.to_string(),
            vendor: "claude-code".into(),
            vendor_session_id: format!("vendor-{key}"),
            root_session_key: key.to_string(),
            lineage_depth: LineageDepth::ROOT,
            project_key: PROJECT.into(),
            scope,
            cwd: Some("/repo".into()),
            title: Some(format!("title {key}")),
            first_message_at_utc: first_ms.map(|ms| format!("ms-{ms}")),
            first_message_at_ms: first_ms,
            last_activity_at_utc: None,
            last_activity_at_ms: None,
            timestamp_confidence: TimestampConfidence::Native,
            message_count: 0,
            tool_count: 0,
            file_path: format!("/repo/{key}.jsonl"),
            source: pointer(&format!("/repo/{key}.jsonl"), 0, b"{}"),
        }
    }

    fn message(
        session_key: &str,
        ordinal: u32,
        role: NormalizedRole,
        text: &str,
        depth: LineageDepth,
    ) -> NormalizedMessage {
        NormalizedMessage {
            schema_version: SCHEMA_VERSION,
            message_key: format!("{session_key}#{ordinal}"),
            session_key: session_key.to_string(),
            root_session_key: session_key.to_string(),
            lineage_depth: depth,
            ordinal,
            role,
            timestamp_utc: Some("2026-09-01T00:00:00Z".into()),
            timestamp_ms: Some(1_756_684_800_000 + i64::from(ordinal)),
            timestamp_confidence: TimestampConfidence::Native,
            text: text.to_string(),
            tool_name: None,
            tool_call_id: None,
            source: pointer(
                &format!("/repo/{session_key}.jsonl"),
                ordinal as u64,
                text.as_bytes(),
            ),
        }
    }

    fn store() -> MemoryStore {
        MemoryStore::open_in_memory(PROJECT).unwrap()
    }

    /// Chinese is the language most of this project's transcripts are written
    /// in, and before the `cjk` column none of these queries returned anything:
    /// `unicode61` makes an unbroken CJK run exactly one token, so only a query
    /// equal to the whole run could match.
    ///
    /// Delete the `cjk` column write in `write_message_row`, or drop the
    /// `{cjk}:` branch of `to_fts_match`, and every assertion below goes to zero
    /// rows.
    #[test]
    fn a_chinese_word_is_found_inside_a_chinese_sentence() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(100), SessionScope::Scoped),
                &[
                    message(
                        "s1",
                        0,
                        NormalizedRole::User,
                        "重构了内存索引的存储层",
                        LineageDepth::ROOT,
                    ),
                    message(
                        "s1",
                        1,
                        NormalizedRole::Assistant,
                        "把 claude/codex/pi 的历史会话统一为标准结构",
                        LineageDepth::ROOT,
                    ),
                ],
                &[],
            )
            .unwrap();

        for (query, expected) in [
            ("内存", "重构了内存索引的存储层"),
            ("内存索引", "重构了内存索引的存储层"),
            ("存储层", "重构了内存索引的存储层"),
            ("会话", "把 claude/codex/pi 的历史会话统一为标准结构"),
            ("历史会话", "把 claude/codex/pi 的历史会话统一为标准结构"),
        ] {
            let hits = store.search(query, false, &[], 10).unwrap();
            assert_eq!(
                hits.len(),
                1,
                "query {query:?} returned {} hits, expected exactly one",
                hits.len()
            );
            assert_eq!(hits[0].text, expected, "query {query:?} matched the wrong row");
        }
    }

    #[test]
    fn a_single_chinese_character_matches_through_a_prefix_term() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(100), SessionScope::Scoped),
                &[message(
                    "s1",
                    0,
                    NormalizedRole::User,
                    "内存索引",
                    LineageDepth::ROOT,
                )],
                &[],
            )
            .unwrap();
        // `内` starts the bigram `内存`, so a prefix term reaches it.
        assert_eq!(store.search("内", false, &[], 10).unwrap().len(), 1);
        // `引` only ever appears as the *second* half of a bigram, which a
        // prefix term cannot reach. Asserted so the limitation is recorded
        // rather than discovered.
        assert_eq!(store.search("引", false, &[], 10).unwrap().len(), 0);
    }

    #[test]
    fn a_mixed_query_matches_its_latin_and_chinese_halves_together() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(100), SessionScope::Scoped),
                &[
                    message(
                        "s1",
                        0,
                        NormalizedRole::ToolResult,
                        "error ENOENT 找不到文件",
                        LineageDepth::ROOT,
                    ),
                    message(
                        "s1",
                        1,
                        NormalizedRole::ToolResult,
                        "error EACCES 权限不足",
                        LineageDepth::ROOT,
                    ),
                ],
                &[],
            )
            .unwrap();
        // Implicit AND across the two columns: only the first row has both.
        let hits = store.search("ENOENT 文件", false, &[], 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "error ENOENT 找不到文件");
        // And a term glued across the boundary splits into one term per column.
        assert_eq!(store.search("ENOENT找不到", false, &[], 10).unwrap().len(), 1);
    }

    #[test]
    fn latin_search_is_unchanged_by_the_cjk_column() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(100), SessionScope::Scoped),
                &[message(
                    "s1",
                    0,
                    NormalizedRole::ToolCall,
                    "git commit --no-verify",
                    LineageDepth::ROOT,
                )],
                &[],
            )
            .unwrap();
        for query in ["no-verify", "--no-verify", "commit", "git commit"] {
            assert_eq!(
                store.search(query, false, &[], 10).unwrap().len(),
                1,
                "latin query {query:?} regressed"
            );
        }
    }

    /// Terms whose leading punctuation is part of the thing being looked for.
    ///
    /// `unicode61` treats punctuation as a separator either way, so `.env`
    /// tokenizes to `env` and finds the dotfile; what matters is that the term
    /// is not dropped and does not become a syntax error inside `MATCH`.
    #[test]
    fn punctuation_carrying_terms_are_still_searchable() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(100), SessionScope::Scoped),
                &[
                    message("s1", 0, NormalizedRole::ToolResult, "cat .env failed", LineageDepth::ROOT),
                    message("s1", 1, NormalizedRole::User, "the C++ build breaks", LineageDepth::ROOT),
                    message("s1", 2, NormalizedRole::ToolCall, "grep -rn \"foo\" src/", LineageDepth::ROOT),
                ],
                &[],
            )
            .unwrap();
        for query in [".env", "C++", "-rn", "\"foo\""] {
            assert!(
                !store.search(query, false, &[], 10).unwrap().is_empty(),
                "query {query:?} found nothing"
            );
        }
    }

    /// A punctuation-only word must not swallow the rest of the query.
    #[test]
    fn punctuation_only_terms_are_dropped_rather_than_matching_nothing() {
        assert_eq!(to_fts_match("::"), None);
        assert_eq!(
            to_fts_match("foo :: bar"),
            Some("{text}: \"foo\" {text}: \"bar\"".to_string())
        );
    }

    /// Reopening a version-1 database must not fail with "table messages_fts
    /// has 1 columns but 2 values were supplied"; it must discard the stale
    /// layout and come back writable.
    #[test]
    fn a_version_one_database_is_rebuilt_rather_than_left_unwritable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("index.sqlite3");
        {
            // Exactly the v1 layout: single-column FTS, no store_version key.
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(META_SQL)
                .and_then(|()| {
                    connection.execute_batch(
                        "CREATE VIRTUAL TABLE messages_fts USING fts5(text, tokenize = 'unicode61');
                         INSERT INTO messages_fts(text) VALUES ('stale row');",
                    )
                })
                .unwrap();
            connection
                .execute(
                    "INSERT INTO meta(key, value) VALUES ('schema_version', '1')",
                    [],
                )
                .unwrap();
        }

        let mut store = MemoryStore::open(&path, PROJECT).unwrap();
        store
            .replace_session(
                &session("s1", Some(100), SessionScope::Scoped),
                &[message(
                    "s1",
                    0,
                    NormalizedRole::User,
                    "内存索引",
                    LineageDepth::ROOT,
                )],
                &[],
            )
            .unwrap();
        assert_eq!(store.search("内存", false, &[], 10).unwrap().len(), 1);
        assert_eq!(
            store.search("stale", false, &[], 10).unwrap().len(),
            0,
            "the v1 row survived the rebuild"
        );
    }

    /// AC9. Schema creation, upsert and a `MATCH` query, end to end, on the
    /// SQLite build this crate actually links.
    #[test]
    fn fts5_index_builds_and_answers_a_match_query() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(100), SessionScope::Scoped),
                &[
                    message(
                        "s1",
                        0,
                        NormalizedRole::User,
                        "fix the login redirect",
                        LineageDepth::ROOT,
                    ),
                    message(
                        "s1",
                        1,
                        NormalizedRole::Assistant,
                        "the redirect loops on refresh",
                        LineageDepth::ROOT,
                    ),
                ],
                &[],
            )
            .unwrap();
        let hits = store.search("redirect", false, &[], 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|hit| hit.text.contains("redirect")));
        assert_eq!(store.counts().unwrap(), (1, 2, 0));
    }

    #[test]
    fn a_database_file_round_trips_on_disk() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("index.sqlite3");
        {
            let mut store = MemoryStore::open(&path, PROJECT).unwrap();
            store
                .replace_session(
                    &session("s1", Some(1), SessionScope::Scoped),
                    &[message(
                        "s1",
                        0,
                        NormalizedRole::User,
                        "persisted text",
                        LineageDepth::ROOT,
                    )],
                    &[],
                )
                .unwrap();
        }
        let reopened = MemoryStore::open(&path, PROJECT).unwrap();
        assert_eq!(
            reopened.search("persisted", false, &[], 10).unwrap().len(),
            1
        );
    }

    /// A database moved into another project's namespace must stop answering
    /// rather than answer with the wrong project's history.
    #[test]
    fn an_index_refuses_to_open_under_a_different_project() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("index.sqlite3");
        MemoryStore::open(&path, PROJECT).unwrap();
        let error = MemoryStore::open(&path, "other-99887766554433221").unwrap_err();
        assert_eq!(error.code, ERR_STORE_FAILED);
        assert!(
            error.detail.contains("belongs to project"),
            "{}",
            error.detail
        );
    }

    /// Re-ingest is delete-then-insert per session, so a second build of the
    /// same session must not duplicate rows or leave orphaned FTS text.
    #[test]
    fn re_ingesting_a_session_replaces_it_rather_than_duplicating_it() {
        let mut store = store();
        let first = [message(
            "s1",
            0,
            NormalizedRole::User,
            "original wording",
            LineageDepth::ROOT,
        )];
        let second = [message(
            "s1",
            0,
            NormalizedRole::User,
            "revised wording",
            LineageDepth::ROOT,
        )];
        store
            .replace_session(&session("s1", Some(1), SessionScope::Scoped), &first, &[])
            .unwrap();
        store
            .replace_session(&session("s1", Some(1), SessionScope::Scoped), &second, &[])
            .unwrap();

        assert_eq!(store.counts().unwrap(), (1, 1, 0));
        assert!(
            store.search("original", false, &[], 10).unwrap().is_empty(),
            "the replaced text must be gone from the full-text index, not just from the metadata"
        );
        assert_eq!(store.search("revised", false, &[], 10).unwrap().len(), 1);
    }

    /// AC4. The ordering the feature was asked for. mtime is deliberately not
    /// involved: `s_old` is written last and would win an mtime ordering.
    #[test]
    fn sessions_are_ordered_by_first_message_time_not_insertion_order() {
        let mut store = store();
        for (key, first_ms) in [
            ("s_mid", Some(200)),
            ("s_new", Some(300)),
            ("s_old", Some(100)),
        ] {
            store
                .replace_session(&session(key, first_ms, SessionScope::Scoped), &[], &[])
                .unwrap();
        }
        let keys: Vec<String> = store
            .list_sessions(false, &[], 10)
            .unwrap()
            .into_iter()
            .map(|entry| entry.session_key)
            .collect();
        assert_eq!(keys, vec!["s_new", "s_mid", "s_old"]);
    }

    /// A session with no known first-message time must not outrank every
    /// session whose time is known.
    #[test]
    fn sessions_with_no_known_first_message_time_sort_last() {
        let mut store = store();
        store
            .replace_session(&session("s_unknown", None, SessionScope::Scoped), &[], &[])
            .unwrap();
        store
            .replace_session(&session("s_known", Some(1), SessionScope::Scoped), &[], &[])
            .unwrap();
        let keys: Vec<String> = store
            .list_sessions(false, &[], 10)
            .unwrap()
            .into_iter()
            .map(|entry| entry.session_key)
            .collect();
        assert_eq!(keys, vec!["s_known", "s_unknown"]);
    }

    /// AC8's storage half: unproven ownership is excluded unless explicitly
    /// requested.
    #[test]
    fn unscoped_sessions_are_excluded_by_default_from_both_queries() {
        let mut store = store();
        store
            .replace_session(
                &session("s_unscoped", Some(1), SessionScope::Unscoped),
                &[message(
                    "s_unscoped",
                    0,
                    NormalizedRole::User,
                    "borrowed context",
                    LineageDepth::ROOT,
                )],
                &[],
            )
            .unwrap();

        assert!(store.list_sessions(false, &[], 10).unwrap().is_empty());
        assert!(store.search("borrowed", false, &[], 10).unwrap().is_empty());
        assert_eq!(store.list_sessions(true, &[], 10).unwrap().len(), 1);
        assert_eq!(store.search("borrowed", true, &[], 10).unwrap().len(), 1);
    }

    /// AC2's storage half. A `NULL` depth column must come back as unknown, not
    /// as root.
    #[test]
    fn an_unknown_lineage_depth_survives_the_database_round_trip() {
        let mut store = store();
        let mut only = session("s1", Some(1), SessionScope::Scoped);
        only.lineage_depth = LineageDepth::UNKNOWN;
        store
            .replace_session(
                &only,
                &[message(
                    "s1",
                    0,
                    NormalizedRole::ToolResult,
                    "agent_job output",
                    LineageDepth::UNKNOWN,
                )],
                &[],
            )
            .unwrap();

        let stored = store.get_session("s1").unwrap().unwrap();
        assert_eq!(stored.lineage_depth, LineageDepth::UNKNOWN);
        assert!(!stored.lineage_depth.is_root());
        let hit = &store.search("agent_job", false, &[], 10).unwrap()[0];
        assert_eq!(hit.lineage_depth, LineageDepth::UNKNOWN);
    }

    /// Tool output is full of characters FTS5 treats as operators. A search for
    /// them has to be a search, not a syntax error.
    #[test]
    fn queries_full_of_fts5_operators_are_treated_as_literal_terms() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(1), SessionScope::Scoped),
                &[message(
                    "s1",
                    0,
                    NormalizedRole::ToolCall,
                    "ran git commit --no-verify and it failed with ENOENT",
                    LineageDepth::ROOT,
                )],
                &[],
            )
            .unwrap();

        for hostile in [
            "--no-verify",
            "\"ENOENT\"",
            "ENOENT*",
            "NEAR(a b)",
            "git OR",
        ] {
            let hits = store
                .search(hostile, false, &[], 10)
                .unwrap_or_else(|error| panic!("{hostile} must not be a syntax error: {error}"));
            let _ = hits;
        }
        assert_eq!(
            store.search("--no-verify", false, &[], 10).unwrap().len(),
            1
        );
        assert_eq!(store.search("\"ENOENT\"", false, &[], 10).unwrap().len(), 1);
    }

    #[test]
    fn an_empty_query_matches_nothing_rather_than_everything() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(1), SessionScope::Scoped),
                &[message(
                    "s1",
                    0,
                    NormalizedRole::User,
                    "anything",
                    LineageDepth::ROOT,
                )],
                &[],
            )
            .unwrap();
        for empty in ["", "   ", "-", "***"] {
            assert!(
                store.search(empty, false, &[], 10).unwrap().is_empty(),
                "{empty:?} must not match every message"
            );
        }
        assert_eq!(to_fts_match(""), None);
        assert_eq!(
            to_fts_match("a b"),
            Some("{text}: \"a\" {text}: \"b\"".to_string())
        );
    }

    #[test]
    fn compactions_are_searchable_and_carry_their_dropped_context_markers() {
        let mut store = store();
        let record = CompactionRecord {
            schema_version: SCHEMA_VERSION,
            session_key: "s1".into(),
            root_session_key: "s1".into(),
            ordinal: 7,
            summary: "summarised the failed migration attempt".into(),
            tokens_before: Some(120_000),
            first_kept_entry_id: Some("entry-42".into()),
            timestamp_utc: Some("2026-09-01T00:00:00Z".into()),
            timestamp_ms: Some(1_756_684_800_000),
            source: pointer("/repo/s1.jsonl", 900, b"{\"type\":\"compaction\"}"),
        };
        store
            .replace_session(
                &session("s1", Some(1), SessionScope::Scoped),
                &[],
                &[record.clone()],
            )
            .unwrap();

        let found = store.search_compactions("migration", false, 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tokens_before, Some(120_000));
        assert_eq!(found[0].first_kept_entry_id.as_deref(), Some("entry-42"));
        assert!(store
            .search_compactions("nonexistent-phrase", false, 10)
            .unwrap()
            .is_empty());
    }

    /// A LIKE wildcard typed by a user must be a literal, or `%` returns the
    /// entire corpus.
    #[test]
    fn compaction_search_escapes_like_wildcards() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(1), SessionScope::Scoped),
                &[],
                &[CompactionRecord {
                    schema_version: SCHEMA_VERSION,
                    session_key: "s1".into(),
                    root_session_key: "s1".into(),
                    ordinal: 0,
                    summary: "plain summary".into(),
                    tokens_before: None,
                    first_kept_entry_id: None,
                    timestamp_utc: None,
                    timestamp_ms: None,
                    source: pointer("/repo/s1.jsonl", 0, b"{}"),
                }],
            )
            .unwrap();
        assert!(
            store.search_compactions("%", false, 10).unwrap().is_empty(),
            "a literal % must not act as a wildcard"
        );
    }

    #[test]
    fn forgetting_a_session_removes_its_text_from_the_full_text_index() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(1), SessionScope::Scoped),
                &[message(
                    "s1",
                    0,
                    NormalizedRole::User,
                    "ephemeral",
                    LineageDepth::ROOT,
                )],
                &[],
            )
            .unwrap();
        assert_eq!(store.forget_sessions(&["s1".to_string()]).unwrap(), 1);
        assert_eq!(store.counts().unwrap(), (0, 0, 0));
        assert!(store.search("ephemeral", true, &[], 10).unwrap().is_empty());
        assert_eq!(store.forget_sessions(&[]).unwrap(), 0);
    }

    #[test]
    fn indexed_identity_drives_the_incremental_skip() {
        let mut store = store();
        assert!(store.indexed_file_identity("s1").unwrap().is_none());
        store
            .replace_session(&session("s1", Some(1), SessionScope::Scoped), &[], &[])
            .unwrap();
        let stored = store.indexed_file_identity("s1").unwrap().unwrap();
        assert!(stored.matches(&identity()));
        assert_eq!(store.all_session_keys().unwrap(), vec!["s1".to_string()]);
    }

    #[test]
    fn session_messages_come_back_in_transcript_order() {
        let mut store = store();
        store
            .replace_session(
                &session("s1", Some(1), SessionScope::Scoped),
                &[
                    message(
                        "s1",
                        2,
                        NormalizedRole::ToolResult,
                        "third",
                        LineageDepth::ROOT,
                    ),
                    message("s1", 0, NormalizedRole::User, "first", LineageDepth::ROOT),
                    message(
                        "s1",
                        1,
                        NormalizedRole::ToolCall,
                        "second",
                        LineageDepth::ROOT,
                    ),
                ],
                &[],
            )
            .unwrap();
        let ordinals: Vec<u32> = store
            .session_messages("s1", 10)
            .unwrap()
            .into_iter()
            .map(|hit| hit.ordinal)
            .collect();
        assert_eq!(ordinals, vec![0, 1, 2]);
    }

    /// The agent filter is optional, and empty means **every** agent — memory
    /// is looked up across agents by default, because which CLI was running
    /// when something was worked out is rarely what you remember about it.
    #[test]
    fn an_empty_agent_filter_searches_every_agent() {
        let mut store = store();
        for (key, vendor) in [
            ("s_claude", "claude-code"),
            ("s_codex", "codex"),
            ("s_pi", "pi"),
        ] {
            let mut entry = session(key, Some(1), SessionScope::Scoped);
            entry.vendor = vendor.to_string();
            store
                .replace_session(
                    &entry,
                    &[message(
                        key,
                        0,
                        NormalizedRole::User,
                        "shared phrase",
                        LineageDepth::ROOT,
                    )],
                    &[],
                )
                .unwrap();
        }

        assert_eq!(store.search("shared", false, &[], 10).unwrap().len(), 3);
        assert_eq!(store.list_sessions(false, &[], 10).unwrap().len(), 3);
    }

    #[test]
    fn an_explicit_agent_filter_narrows_without_excluding_the_rest_by_default() {
        let mut store = store();
        for (key, vendor) in [("s_claude", "claude-code"), ("s_pi", "pi")] {
            let mut entry = session(key, Some(1), SessionScope::Scoped);
            entry.vendor = vendor.to_string();
            store
                .replace_session(
                    &entry,
                    &[message(
                        key,
                        0,
                        NormalizedRole::User,
                        "shared phrase",
                        LineageDepth::ROOT,
                    )],
                    &[],
                )
                .unwrap();
        }

        let only_pi = store
            .search("shared", false, &[MemoryVendor::Pi], 10)
            .unwrap();
        assert_eq!(only_pi.len(), 1);
        assert_eq!(only_pi[0].vendor, "pi");

        let two = store
            .search(
                "shared",
                false,
                &[MemoryVendor::Pi, MemoryVendor::ClaudeCode],
                10,
            )
            .unwrap();
        assert_eq!(two.len(), 2);

        let listed = store
            .list_sessions(false, &[MemoryVendor::ClaudeCode], 10)
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].vendor, "claude-code");
    }

    /// An MCP client must not be able to pull the whole corpus through one call.
    #[test]
    fn query_limits_are_clamped_at_both_ends() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(10), 10);
        assert_eq!(clamp_limit(usize::MAX), MAX_QUERY_LIMIT);
    }
}
