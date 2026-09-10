//! Cross-agent conversation memory index.
//!
//! Scans the historical transcripts written by three CLI agents (Claude Code,
//! Codex, pi), normalizes user messages / assistant replies / tool calls into
//! one shape, and stores a searchable projection in a host-private SQLite+FTS5
//! index namespaced per project. The index is then exposed over MCP as a
//! project-scoped knowledge base.
//!
//! Three properties this module exists to guarantee, each reversing a mistake
//! the design analysis caught:
//!
//! 1. **The index is host-private.** It lives under the host's own state root
//!    (`app_data_dir` on desktop, the service-account state dir standalone),
//!    never inside the user's project directory — the same rule Termul's own
//!    conversations already follow. See [`paths`].
//! 2. **One project at a time.** [`scope::ProjectFence`] holds exactly one
//!    canonical project root. The existing `cli_session` scope helper walks
//!    *every* non-archived project by design; reusing it here would serve one
//!    project's transcripts as another project's memory.
//! 3. **Hierarchy is a message label, not a storage shape.** Subagent
//!    transcripts are flattened; each message carries a
//!    [`types::LineageDepth`] taken from that vendor's own mechanism, and
//!    `None` when the vendor does not record one. Guessing `0` there would
//!    turn "unknown" into the false claim "this is a root message".

pub mod adapters;
pub mod ingest;
pub mod paths;
pub mod redact;
pub mod scope;
pub mod service;
pub mod stdio_mcp;
pub mod store;
pub mod types;

pub use paths::{CanonicalProjectRoot, IndexLocation, MemoryVendor};
pub use scope::ProjectFence;
pub use store::{MemorySearchHit, MemoryStore, StoreWriteReport};
pub use types::{
    CompactionRecord, FileIdentity, IndexedSession, LineageDepth, NormalizedMessage,
    NormalizedRole, PointerFreshness, SessionScope, SourcePointer, TimestampConfidence,
};

/// Stable error codes shared by the Tauri, HTTP and MCP surfaces.
///
/// Kept as `&'static str` codes rather than an opaque message so the three
/// transports can report the same failure identically (the repo keeps stable
/// error codes across Tauri/HTTP/WS by convention).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryIndexError {
    pub code: &'static str,
    pub detail: String,
}

impl MemoryIndexError {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for MemoryIndexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for MemoryIndexError {}

pub type MemoryIndexResult<T> = Result<T, MemoryIndexError>;

/// The index root is outside the project, the project root is not a file, the
/// requested scope is a single project — all of these are caller mistakes with
/// distinct meanings, so they get distinct codes.
pub const ERR_PROJECT_ROOT_INVALID: &str = "MEMORY_INDEX_PROJECT_ROOT_INVALID";
pub const ERR_STATE_ROOT_INVALID: &str = "MEMORY_INDEX_STATE_ROOT_INVALID";
pub const ERR_STATE_ROOT_INSIDE_PROJECT: &str = "MEMORY_INDEX_STATE_ROOT_INSIDE_PROJECT";
pub const ERR_OUT_OF_SCOPE: &str = "MEMORY_INDEX_OUT_OF_SCOPE";
pub const ERR_STORE_FAILED: &str = "MEMORY_INDEX_STORE_FAILED";
pub const ERR_SOURCE_STALE: &str = "MEMORY_INDEX_SOURCE_STALE";
pub const ERR_INGEST_FAILED: &str = "MEMORY_INDEX_INGEST_FAILED";
