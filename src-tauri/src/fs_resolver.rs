//! Batch resolution of reported file paths.
//!
//! Callers (external "open with" launches, editor "go to file" from a stack
//! trace, diff headers, terminal link output) hand us a *reported* path that
//! may not be a usable absolute path: it can be a repo-relative diff path
//! (`a/src/lib.rs`), a path relative to some cwd (`lib/foo.ts`), a `~/`-prefixed
//! path, or a suffix of a real path inside a monorepo. This module turns such
//! candidates into existing absolute file paths, deterministically and without
//! ever probing outside the caller's boundary.
//!
//! Two boundaries exist:
//! - The desktop caller passes `allowed_roots = None`: any file the user can
//!   read is fair game, matching the directory-picker breadth of `/fs/*`.
//! - The web/remote caller passes `Some(registered_roots)`: a candidate that
//!   resolves outside every registered project root is dropped, so a remote
//!   client cannot use this route as an existence oracle for the host's disk.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Upper bound on candidates per request. Each candidate can cost filesystem
/// stat calls; the cap keeps a single request bounded.
pub const MAX_CANDIDATES: usize = 512;
/// Upper bound on roots per request.
const MAX_ROOTS: usize = 64;
/// Longest accepted candidate, in bytes. Rejects pathological inputs early.
const MAX_CANDIDATE_LEN: usize = 4096;
/// Directory-name segments whose subtrees are skipped by the suffix scan.
const SCAN_IGNORED_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".next",
    ".cache",
    ".turbo",
    ".output",
    ".nuxt",
    ".svelte-kit",
    "coverage",
    ".nyc_output",
    "__pycache__",
    ".venv",
    "venv",
    "vendor",
    "Pods",
];
/// Deepest directory level the suffix scan descends to.
const SCAN_MAX_DEPTH: usize = 8;
/// Most filesystem entries the suffix scan inspects per candidate.
const SCAN_MAX_ENTRIES: usize = 20_000;
/// Wall-clock budget per candidate for the suffix scan.
const SCAN_BUDGET: Duration = Duration::from_millis(750);
/// Total wall-clock budget across all suffix scans in a single request.
const TOTAL_SCAN_BUDGET: Duration = Duration::from_millis(1000);

/// Request shape mirrors `ResolveFilePathsRequest` in
/// `src/shared/types/filesystem.types.ts`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveFilePathsRequest {
    pub candidates: Vec<String>,
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub expand_home: Option<bool>,
}

/// One `candidate -> resolvedPath` pair. Mirrors `ResolvedFilePathHit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedFilePathHit {
    pub candidate: String,
    pub resolved_path: String,
}

/// Mirrors `ResolveFilePathsResponse`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveFilePathsResponse {
    pub hits: Vec<ResolvedFilePathHit>,
}

/// Resolve `request.candidates` against `request.roots`.
///
/// A candidate that resolves to an existing regular file yields one hit; every
/// other candidate is silently dropped. Order of attempts per candidate:
///
/// 1. absolute path, checked directly;
/// 2. relative path joined onto each root in order, `..` folded lexically;
/// 3. the same two steps again with a leading diff prefix (`a/`, `b/`) removed;
/// 4. a bounded suffix scan under the last root when the candidate has at least
///    one path separator, accepting only a single unambiguous match.
///
/// `allowed_roots` is the hard boundary: when present, a resolved path must live
/// under one of them or the candidate is dropped.
#[must_use]
pub fn resolve_existing_paths(
    request: ResolveFilePathsRequest,
    allowed_roots: Option<&[PathBuf]>,
) -> ResolveFilePathsResponse {
    let expand_home = request.expand_home.unwrap_or(true);
    let roots = normalize_roots(&request.roots);
    let allowed = allowed_roots.map(normalize_root_slice);

    let mut hits = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let request_started = Instant::now();
    for raw in request.candidates.into_iter().take(MAX_CANDIDATES) {
        let Some(candidate) = normalize_candidate(&raw) else {
            continue;
        };
        if !seen.insert(candidate.clone()) {
            continue;
        }

        let mut resolved = None;
        for attempt in candidate_variants(&candidate) {
            if let Some(hit) = resolve_direct(&attempt, &roots, allowed.as_deref(), expand_home) {
                resolved = Some(hit);
                break;
            }
            if let Some(hit) = resolve_relative(&attempt, &roots, allowed.as_deref(), expand_home) {
                resolved = Some(hit);
                break;
            }
        }

        if resolved.is_none()
            && candidate.contains('/')
            && request_started.elapsed() < TOTAL_SCAN_BUDGET
        {
            if let Some(scan_root) = roots.last() {
                let remaining = TOTAL_SCAN_BUDGET.saturating_sub(request_started.elapsed());
                let budget = remaining.min(SCAN_BUDGET);
                for attempt in candidate_variants(&candidate) {
                    if let Some(path) =
                        suffix_scan(scan_root, &attempt, allowed.as_deref(), budget)
                    {
                        resolved = Some(path);
                        break;
                    }
                }
            }
        }

        if let Some(path) = resolved {
            hits.push(ResolvedFilePathHit {
                candidate: raw,
                resolved_path: path.to_string_lossy().into_owned(),
            });
        }
    }
    ResolveFilePathsResponse { hits }
}

/// Normalize one raw argument into a slash-separated candidate, or `None` when
/// it cannot name a file. Strips the wrappers a terminal/markdown capture tends
/// to leave behind (backticks, quotes, `<>`, a trailing `:` line suffix).
#[must_use]
pub fn normalize_candidate(raw: &str) -> Option<String> {
    let mut value = raw.trim();
    if value.len() > MAX_CANDIDATE_LEN {
        return None;
    }
    value = value.trim_matches(|c| matches!(c, '`' | '"' | '\''));
    value = value.trim_start_matches('<').trim_end_matches('>');
    // `src/lib.rs:42` and `src/lib.rs:42:7` from a compiler/stack trace.
    value = strip_trailing_line_suffix(value);
    let value = value.replace('\\', "/");
    // `./readme.md` and `readme.md` name the same file, so they must share one
    // dedupe key. `../` is intentionally left alone: it is not redundant.
    let mut value = value.trim();
    while let Some(rest) = value.strip_prefix("./") {
        value = rest;
    }
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_string())
}

/// Drop a leading unified-diff prefix (`a/`, `b/`) when one is present and the
/// remainder still differs.
#[must_use]
pub fn strip_diff_prefix(candidate: &str) -> Option<String> {
    for prefix in ["a/", "b/"] {
        if let Some(rest) = candidate.strip_prefix(prefix) {
            let rest = rest.trim_start_matches('/');
            if !rest.is_empty() && rest != candidate {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn strip_trailing_line_suffix(value: &str) -> &str {
    // Only strip a `:N` or `:N:M` numeric tail, never a Windows drive colon.
    let Some(colon) = value.rfind(':') else {
        return value;
    };
    let tail = &value[colon + 1..];
    if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
        return value;
    }
    let head = &value[..colon];
    // `C:` alone (drive letter) must survive.
    if head.len() == 1 && head.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return value;
    }
    match head.rfind(':') {
        Some(inner) => {
            let inner_tail = &head[inner + 1..];
            if !inner_tail.is_empty() && inner_tail.chars().all(|c| c.is_ascii_digit()) {
                &head[..inner]
            } else {
                head
            }
        }
        None => head,
    }
}

fn candidate_variants(candidate: &str) -> Vec<String> {
    let mut variants = vec![candidate.to_string()];
    if let Some(stripped) = strip_diff_prefix(candidate) {
        variants.push(stripped);
    }
    variants
}

fn normalize_roots(roots: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots.iter().take(MAX_ROOTS) {
        let Some(value) = normalize_candidate(root) else {
            continue;
        };
        let Ok(path) = expand_candidate(&value, true) else {
            continue;
        };
        let cleaned = lexical_normalize(&path);
        if cleaned.is_absolute() && !out.contains(&cleaned) {
            out.push(cleaned);
        }
    }
    out
}

fn normalize_root_slice(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter(|root| !root.as_os_str().is_empty())
        .map(|root| {
            std::fs::canonicalize(root).unwrap_or_else(|_| lexical_normalize(root))
        })
        .filter(|root| !root.as_os_str().is_empty() && root.is_absolute())
        .collect()
}

/// Expand `~/` and return the path unchanged otherwise.
fn expand_candidate(value: &str, expand_home: bool) -> Result<PathBuf, ()> {
    if expand_home {
        if value == "~" {
            return se_mcp_bridge::settings::home_dir().ok_or(());
        }
        if let Some(rest) = value.strip_prefix("~/") {
            let home = se_mcp_bridge::settings::home_dir().ok_or(())?;
            return Ok(home.join(rest));
        }
    }
    Ok(PathBuf::from(value))
}

/// Fold `.` and `..` lexically without touching the filesystem.
#[must_use]
pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    if !path.is_absolute() {
                        out.push("..");
                    }
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() && path.is_absolute() {
        out.push("/");
    }
    out
}

fn resolve_direct(
    candidate: &str,
    _roots: &[PathBuf],
    allowed: Option<&[PathBuf]>,
    expand_home: bool,
) -> Option<PathBuf> {
    let path = expand_candidate(candidate, expand_home).ok()?;
    if !path.is_absolute() {
        return None;
    }
    accept_existing(&lexical_normalize(&path), allowed)
}

fn resolve_relative(
    candidate: &str,
    roots: &[PathBuf],
    allowed: Option<&[PathBuf]>,
    expand_home: bool,
) -> Option<PathBuf> {
    let path = expand_candidate(candidate, expand_home).ok()?;
    if path.is_absolute() {
        return None;
    }
    for root in roots {
        let joined = lexical_normalize(&root.join(&path));
        if let Some(hit) = accept_existing(&joined, allowed) {
            return Some(hit);
        }
    }
    None
}

fn accept_existing(path: &Path, allowed: Option<&[PathBuf]>) -> Option<PathBuf> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        _ => return None,
    }
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !within_boundary(&resolved, allowed) {
        return None;
    }
    Some(resolved)
}

fn within_boundary(path: &Path, allowed: Option<&[PathBuf]>) -> bool {
    let Some(roots) = allowed else {
        return true;
    };
    if roots.is_empty() {
        return false;
    }
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    roots.iter().any(|root| {
        if root.as_os_str().is_empty() {
            return false;
        }
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        !root.as_os_str().is_empty() && root.is_absolute() && canonical.starts_with(&root)
    })
}

/// Breadth-first search under `root` for a path whose relative form ends with
/// `suffix` on a path boundary. Returns only a unique match.
fn suffix_scan(
    root: &Path,
    suffix: &str,
    allowed: Option<&[PathBuf]>,
    budget: Duration,
) -> Option<PathBuf> {
    if !root.is_dir() || budget.is_zero() {
        return None;
    }
    let suffix = suffix.trim_start_matches("./").trim_start_matches('/');
    if suffix.is_empty() {
        return None;
    }
    let started = Instant::now();
    let mut inspected = 0usize;
    let mut queue: std::collections::VecDeque<(PathBuf, String, usize)> =
        std::collections::VecDeque::new();
    queue.push_back((root.to_path_buf(), String::new(), 0));
    let mut matches: Vec<PathBuf> = Vec::new();

    while let Some((dir, prefix, depth)) = queue.pop_front() {
        if started.elapsed() > budget {
            return None;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::PermissionDenied => continue,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            inspected += 1;
            if inspected > SCAN_MAX_ENTRIES {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && name != ".github" {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if file_type.is_dir() {
                if SCAN_IGNORED_DIRS.contains(&name.as_str()) || depth + 1 > SCAN_MAX_DEPTH {
                    continue;
                }
                queue.push_back((entry.path(), rel, depth + 1));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if rel == suffix || rel.ends_with(&format!("/{suffix}")) {
                let path = std::fs::canonicalize(entry.path())
                    .unwrap_or_else(|_| entry.path());
                if !within_boundary(&path, allowed) {
                    continue;
                }
                if !matches.iter().any(|existing| existing == &path) {
                    matches.push(path);
                }
                if matches.len() > 1 {
                    // Ambiguous: two different files share the suffix.
                    return None;
                }
            }
        }
    }
    matches.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(root: &Path, rel: &str) -> PathBuf {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&path, "x").expect("write file");
        path
    }

    fn request(candidates: &[&str], roots: &[&Path]) -> ResolveFilePathsRequest {
        ResolveFilePathsRequest {
            candidates: candidates.iter().map(|c| (*c).to_string()).collect(),
            roots: roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
            expand_home: Some(false),
        }
    }

    #[test]
    fn resolves_an_absolute_path() {
        let dir = TempDir::new().expect("temp dir");
        let file = write_file(dir.path(), "notes/todo.md");

        let response = resolve_existing_paths(
            request(&[&file.to_string_lossy()], &[dir.path()]),
            None,
        );

        assert_eq!(response.hits.len(), 1);
        assert_eq!(response.hits[0].candidate, file.to_string_lossy());
        assert_eq!(
            PathBuf::from(&response.hits[0].resolved_path),
            file.canonicalize().expect("canonical")
        );
    }

    #[test]
    fn resolves_relative_path_crossing_parent_directory() {
        let dir = TempDir::new().expect("temp dir");
        let cwd = dir.path().join("packages/app");
        fs::create_dir_all(&cwd).expect("cwd");
        let file = write_file(dir.path(), "shared/config/settings.json");

        let response = resolve_existing_paths(
            request(&["../../shared/config/settings.json"], &[&cwd]),
            None,
        );

        assert_eq!(response.hits.len(), 1);
        assert_eq!(
            PathBuf::from(&response.hits[0].resolved_path),
            file.canonicalize().expect("canonical")
        );
    }

    #[test]
    fn strips_a_and_b_diff_prefixes() {
        let dir = TempDir::new().expect("temp dir");
        let file = write_file(dir.path(), "src/lib/renderer.ts");

        let response = resolve_existing_paths(
            request(&["a/src/lib/renderer.ts", "b/src/lib/renderer.ts"], &[dir.path()]),
            None,
        );

        assert_eq!(response.hits.len(), 2);
        for hit in &response.hits {
            assert_eq!(
                PathBuf::from(&hit.resolved_path),
                file.canonicalize().expect("canonical"),
                "candidate {} should resolve to the same file",
                hit.candidate
            );
        }
    }

    #[test]
    fn resolves_unique_suffix_inside_a_nested_package() {
        let dir = TempDir::new().expect("temp dir");
        let file = write_file(dir.path(), "packages/app/src/lib/foo.ts");

        let response =
            resolve_existing_paths(request(&["lib/foo.ts"], &[dir.path()]), None);

        assert_eq!(response.hits.len(), 1);
        assert_eq!(
            PathBuf::from(&response.hits[0].resolved_path),
            file.canonicalize().expect("canonical")
        );
    }

    #[test]
    fn refuses_ambiguous_suffix_matches() {
        let dir = TempDir::new().expect("temp dir");
        write_file(dir.path(), "packages/app/src/lib/foo.ts");
        write_file(dir.path(), "packages/web/src/lib/foo.ts");

        let response =
            resolve_existing_paths(request(&["lib/foo.ts"], &[dir.path()]), None);

        assert!(
            response.hits.is_empty(),
            "two matches must not resolve: {:?}",
            response.hits
        );
    }

    #[test]
    fn refuses_paths_outside_the_allowed_roots() {
        let allowed = TempDir::new().expect("allowed");
        let outside = TempDir::new().expect("outside");
        let outside_file = write_file(outside.path(), "secret.md");
        let inside_file = write_file(allowed.path(), "visible.md");
        let roots = [allowed.path().to_path_buf()];

        let response = resolve_existing_paths(
            request(
                &[&outside_file.to_string_lossy(), &inside_file.to_string_lossy()],
                &[allowed.path()],
            ),
            Some(&roots),
        );

        assert_eq!(response.hits.len(), 1);
        assert_eq!(
            PathBuf::from(&response.hits[0].resolved_path),
            inside_file.canonicalize().expect("canonical")
        );
    }

    #[test]
    fn refuses_traversal_that_escapes_the_allowed_boundary() {
        let allowed = TempDir::new().expect("allowed");
        let sub = allowed.path().join("project");
        fs::create_dir_all(&sub).expect("project dir");
        let outside = TempDir::new().expect("outside");
        write_file(outside.path(), "escape.md");
        let roots = [sub.clone()];

        let response = resolve_existing_paths(
            request(&["../../escape.md"], &[allowed.path()]),
            Some(&roots),
        );

        assert!(response.hits.is_empty(), "traversal must be refused");
    }

    #[test]
    fn ignores_candidates_that_do_not_exist() {
        let dir = TempDir::new().expect("temp dir");
        let response = resolve_existing_paths(
            request(&["missing.md", "--flag", ""], &[dir.path()]),
            None,
        );
        assert!(response.hits.is_empty());
    }

    #[test]
    fn normalizes_backslashes_and_line_suffixes() {
        let dir = TempDir::new().expect("temp dir");
        let file = write_file(dir.path(), "src/app/main.ts");

        let response = resolve_existing_paths(
            request(&["src\\app\\main.ts:42:7"], &[dir.path()]),
            None,
        );

        assert_eq!(response.hits.len(), 1);
        assert_eq!(
            PathBuf::from(&response.hits[0].resolved_path),
            file.canonicalize().expect("canonical")
        );
    }

    #[test]
    fn deduplicates_repeated_candidates() {
        let dir = TempDir::new().expect("temp dir");
        write_file(dir.path(), "readme.md");

        let response =
            resolve_existing_paths(request(&["readme.md", "./readme.md"], &[dir.path()]), None);

        assert_eq!(response.hits.len(), 1, "got {:?}", response.hits);
    }

    #[test]
    fn lexical_normalize_folds_dot_segments() {
        assert_eq!(
            lexical_normalize(Path::new("/a/b/../c/./d.md")),
            PathBuf::from("/a/c/d.md")
        );
        assert_eq!(lexical_normalize(Path::new("/a/./b")), PathBuf::from("/a/b"));
    }

    #[test]
    fn strip_diff_prefix_only_handles_a_and_b() {
        assert_eq!(strip_diff_prefix("a/src/x.ts").as_deref(), Some("src/x.ts"));
        assert_eq!(strip_diff_prefix("b/x.ts").as_deref(), Some("x.ts"));
        assert_eq!(strip_diff_prefix("src/x.ts"), None);
        assert_eq!(strip_diff_prefix("ab/x.ts"), None);
    }

    #[test]
    fn empty_allowed_root_never_grants_access() {
        let dir = TempDir::new().expect("temp dir");
        let file = write_file(dir.path(), "secret.md");
        let raw = file.to_string_lossy().into_owned();

        let response = resolve_existing_paths(
            request(&[&raw], &[dir.path()]),
            Some(&[PathBuf::new()]),
        );

        assert!(
            response.hits.is_empty(),
            "empty allowed root must fail closed, got {:?}",
            response.hits
        );
    }

    #[test]
    fn echoes_raw_candidate_for_dot_slash_and_line_suffix() {
        let dir = TempDir::new().expect("temp dir");
        write_file(dir.path(), "scripts/build.sh");

        let response = resolve_existing_paths(
            request(&["./scripts/build.sh:10"], &[dir.path()]),
            None,
        );

        assert_eq!(response.hits.len(), 1);
        assert_eq!(response.hits[0].candidate, "./scripts/build.sh:10");
    }
}
