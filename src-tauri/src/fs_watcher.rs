//! Recursive filesystem watcher service.
//!
//! Replaces the renderer-driven, per-expanded-directory watching model. The old
//! model made "which directories the file explorer has expanded" decide the OS
//! watcher set, so every expand/collapse/project-switch rebuilt the underlying
//! watcher — and on macOS the release of a `notify` watcher joins its FSEvents
//! thread with no timeout. A spindump caught that join holding the AppKit main
//! thread for 2.31s (`Slow response to HID event`) during an ordinary project
//! switch.
//!
//! This service takes the shape VSCode uses instead:
//!
//! * **Roots, not directories.** The watched set is a small list of project /
//!   workspace roots watched *recursively*. Expanding a folder in the tree is a
//!   pure UI operation that touches no OS resource.
//! * **Filter before the boundary.** Excluded paths are dropped here, in the
//!   host, so they never cross IPC. `tauri-plugin-fs` has no exclude support at
//!   all (`WatchOptions` is only `{ baseDir, recursive, delayMs }`), which is
//!   precisely why this cannot live in the plugin.
//! * **Coalesce and bound.** Events are merged per path over a short quiet
//!   window and delivered in bounded batches, so a build or an `npm install`
//!   cannot flood the renderer.
//! * **Never join on the UI thread.** Replacing the watcher hands the old one to
//!   the blocking pool to drop.

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;

/// How long a path stays quiet before its pending change is delivered.
///
/// Mirrors VSCode's `FILE_CHANGES_HANDLER_DELAY`, which sits at 75ms explicitly
/// to land *after* the 50ms its own watcher backend already applies.
const COALESCE_WINDOW: Duration = Duration::from_millis(75);

/// Most events delivered in one batch.
const MAX_BATCH: usize = 500;

/// Most events held in memory before new ones are dropped.
///
/// Overflow is reported rather than silenced: a user whose build output is
/// flooding the watcher needs to know to exclude it, which is the same call
/// VSCode makes when its `ThrottledWorker` saturates.
const MAX_BUFFERED: usize = 30_000;

/// Path segments never worth watching.
///
/// Matched against whole path components, not as globs. VSCode's own config
/// comment warns that `**` patterns compile to RegExps complex enough to slow
/// large workspaces down; segment equality is both faster and sufficient here.
///
/// Deliberately NOT the whole of `.git`. Git status refreshes on `HEAD` and
/// `index` changes, so excluding the directory wholesale would break branch
/// switch detection. Only the two subtrees that churn are excluded — the same
/// two VSCode ships in its `files.watcherExclude` default.
const EXCLUDED_SEGMENTS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "coverage",
    ".next",
    ".turbo",
    ".cache",
    ".venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
];

/// `.git` subtrees that churn without ever being interesting.
const EXCLUDED_GIT_SUBDIRS: &[&str] = &["objects", "subtree-cache", "lfs"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileChangeKind {
    Add,
    Change,
    Unlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeEvent {
    pub kind: FileChangeKind,
    pub path: String,
}

/// Whether a path is inside something we never want events from.
///
/// Note the `.git` special case: `.git/objects/**` and friends are excluded, but
/// `.git/HEAD` and `.git/index` are not — those are how a branch switch becomes
/// visible.
pub fn is_excluded(path: &Path) -> bool {
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(raw) = component else {
            continue;
        };
        let Some(name) = raw.to_str() else {
            continue;
        };

        if EXCLUDED_SEGMENTS.contains(&name) {
            return true;
        }

        if name == ".git" {
            if let Some(Component::Normal(next_raw)) = components.peek() {
                if let Some(next) = next_raw.to_str() {
                    if EXCLUDED_GIT_SUBDIRS.contains(&next) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Drop roots already covered by another root.
///
/// Watching `/a` recursively already delivers everything under `/a/b`, so
/// registering both wastes an OS watcher and duplicates every nested event.
/// VSCode does the same with a `TernarySearchTree`; with the handful of roots a
/// workspace has, a sort plus prefix test is simpler and just as exact.
///
/// Sorting by component count first means a parent is always considered before
/// its children, so one pass is enough.
pub fn dedupe_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut sorted: Vec<PathBuf> = Vec::new();
    let mut candidates: Vec<&PathBuf> = roots.iter().collect();
    candidates.sort_by_key(|path| path.components().count());

    for candidate in candidates {
        if candidate.as_os_str().is_empty() {
            continue;
        }
        // `starts_with` is component-wise, so `/a/bc` is correctly NOT treated
        // as living under `/a/b`.
        let covered = sorted.iter().any(|kept: &PathBuf| candidate.starts_with(kept));
        if !covered && !sorted.iter().any(|kept| kept == candidate) {
            sorted.push(candidate.clone());
        }
    }
    sorted
}

fn classify(kind: &EventKind) -> FileChangeKind {
    match kind {
        EventKind::Create(_) => FileChangeKind::Add,
        EventKind::Remove(_) => FileChangeKind::Unlink,
        _ => FileChangeKind::Change,
    }
}

/// One merged batch ready for the renderer.
type Batch = Vec<FileChangeEvent>;

struct Coalescer {
    /// Insertion-ordered by `order`, so a burst arrives in the order it happened
    /// rather than in hash order.
    pending: HashMap<String, (FileChangeKind, u64)>,
    order: u64,
    overflow_reported: bool,
}

impl Coalescer {
    fn new() -> Self {
        Self {
            pending: HashMap::new(),
            order: 0,
            overflow_reported: false,
        }
    }

    fn push(&mut self, event: FileChangeEvent) {
        if self.pending.len() >= MAX_BUFFERED && !self.pending.contains_key(&event.path) {
            if !self.overflow_reported {
                self.overflow_reported = true;
                log::warn!(
                    "[fs-watcher] dropping events: {} pending, most recent {}. Exclude high-churn directories to reduce the volume.",
                    self.pending.len(),
                    event.path
                );
            }
            return;
        }

        let order = self.order;
        self.order += 1;
        // A later event supersedes an earlier one for the same path but keeps
        // its original position, so "created then modified" still reads as one
        // change at the point the file first appeared.
        self.pending
            .entry(event.path)
            .and_modify(|slot| slot.0 = event.kind)
            .or_insert((event.kind, order));
    }

    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Take up to `MAX_BATCH` events, oldest first. Anything beyond the cap
    /// stays pending for the next flush rather than being dropped.
    fn drain_batch(&mut self) -> Batch {
        let mut entries: Vec<(String, FileChangeKind, u64)> = self
            .pending
            .iter()
            .map(|(path, (kind, order))| (path.clone(), *kind, *order))
            .collect();
        entries.sort_by_key(|(_, _, order)| *order);
        entries.truncate(MAX_BATCH);

        let mut batch = Batch::with_capacity(entries.len());
        for (path, kind, _) in entries {
            self.pending.remove(&path);
            batch.push(FileChangeEvent { kind, path });
        }
        if self.pending.is_empty() {
            self.overflow_reported = false;
        }
        batch
    }
}

struct Inner {
    watcher: Option<RecommendedWatcher>,
    roots: Vec<PathBuf>,
    sink: Option<Channel<Batch>>,
}

pub struct FsWatcherService {
    inner: Mutex<Inner>,
    tx: Sender<FileChangeEvent>,
}

impl FsWatcherService {
    pub fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<FileChangeEvent>();
        let service = Arc::new(Self {
            inner: Mutex::new(Inner {
                watcher: None,
                roots: Vec::new(),
                sink: None,
            }),
            tx,
        });

        let weak = Arc::downgrade(&service);
        // A dedicated thread rather than an async task: `recv_timeout` is a
        // blocking wait, and the quiet-window semantics are easier to get right
        // without a timer future per burst.
        std::thread::Builder::new()
            .name("fs-watcher-coalescer".into())
            .spawn(move || {
                let mut coalescer = Coalescer::new();
                let mut deadline: Option<Instant> = None;

                loop {
                    let timeout = match deadline {
                        Some(at) => at.saturating_duration_since(Instant::now()),
                        None => Duration::from_millis(250),
                    };

                    match rx.recv_timeout(timeout) {
                        Ok(event) => {
                            coalescer.push(event);
                            deadline = Some(Instant::now() + COALESCE_WINDOW);
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        // Every sender is gone: the service was dropped.
                        Err(RecvTimeoutError::Disconnected) => break,
                    }

                    let due = deadline.is_some_and(|at| Instant::now() >= at);
                    if !due || coalescer.is_empty() {
                        if coalescer.is_empty() {
                            deadline = None;
                        }
                        continue;
                    }

                    let batch = coalescer.drain_batch();
                    deadline = if coalescer.is_empty() {
                        None
                    } else {
                        // More than one batch worth was pending; come straight
                        // back for the rest instead of waiting out another
                        // quiet window.
                        Some(Instant::now())
                    };

                    let Some(service) = weak.upgrade() else { break };
                    let sink = service
                        .inner
                        .lock()
                        .ok()
                        .and_then(|inner| inner.sink.clone());
                    if let Some(sink) = sink {
                        if let Err(err) = sink.send(batch) {
                            log::warn!("[fs-watcher] failed to deliver batch: {err}");
                        }
                    }
                }
            })
            .expect("spawn fs-watcher coalescer thread");

        service
    }

    pub fn set_sink(&self, channel: Channel<Batch>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.sink = Some(channel);
        }
    }

    /// Point the watcher at exactly `roots` (recursively), replacing whatever it
    /// watched before.
    ///
    /// Builds the replacement before releasing the old one so no event window is
    /// lost, and hands the old watcher to the blocking pool to drop — dropping a
    /// `notify` watcher joins its backend thread, which must never happen on the
    /// thread servicing the UI.
    pub fn set_roots(&self, roots: Vec<PathBuf>) -> Result<(), String> {
        let desired = dedupe_roots(&roots);

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "fs watcher state poisoned".to_string())?;

        if inner.roots == desired {
            return Ok(());
        }

        if desired.is_empty() {
            let previous = inner.watcher.take();
            inner.roots.clear();
            drop(inner);
            release_watcher(previous);
            return Ok(());
        }

        let tx = self.tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else { return };
                let kind = classify(&event.kind);
                // Every path, not just the first. A rename arrives as one event
                // carrying both the old and the new path; reading only `paths[0]`
                // silently lost the other end of it.
                for path in event.paths {
                    if is_excluded(&path) {
                        continue;
                    }
                    let Some(path) = path.to_str() else { continue };
                    let _ = tx.send(FileChangeEvent {
                        kind,
                        path: path.replace('\\', "/"),
                    });
                }
            },
            Config::default(),
        )
        .map_err(|err| format!("failed to create fs watcher: {err}"))?;

        for root in &desired {
            watcher
                .watch(root, RecursiveMode::Recursive)
                .map_err(|err| format!("failed to watch {}: {err}", root.display()))?;
        }

        let previous = inner.watcher.replace(watcher);
        inner.roots = desired;
        drop(inner);
        release_watcher(previous);
        Ok(())
    }
}

/// Drop a watcher somewhere that can afford to block.
fn release_watcher(watcher: Option<RecommendedWatcher>) {
    let Some(watcher) = watcher else { return };
    tauri::async_runtime::spawn_blocking(move || drop(watcher));
}

#[tauri::command]
pub async fn fs_watcher_subscribe(
    service: tauri::State<'_, Arc<FsWatcherService>>,
    on_event: Channel<Vec<FileChangeEvent>>,
) -> Result<(), String> {
    service.set_sink(on_event);
    Ok(())
}

#[tauri::command]
pub async fn fs_watcher_set_roots(
    service: tauri::State<'_, Arc<FsWatcherService>>,
    roots: Vec<String>,
) -> Result<(), String> {
    let paths = roots
        .into_iter()
        .filter(|root| !root.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    service.set_roots(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_heavy_build_directories() {
        assert!(is_excluded(Path::new("/p/node_modules/x/index.js")));
        assert!(is_excluded(Path::new("/p/target/debug/app")));
        assert!(is_excluded(Path::new("/p/sub/dist/bundle.js")));
        assert!(is_excluded(Path::new("/p/__pycache__/m.pyc")));
    }

    #[test]
    fn keeps_ordinary_source_paths() {
        assert!(!is_excluded(Path::new("/p/src/main.rs")));
        assert!(!is_excluded(Path::new("/p/src/dist-helper.ts")));
        // Substring, not a segment: must not be excluded.
        assert!(!is_excluded(Path::new("/p/my_node_modules_notes.md")));
    }

    #[test]
    fn excludes_churning_git_subtrees_but_keeps_head_and_index() {
        assert!(is_excluded(Path::new("/p/.git/objects/ab/cdef")));
        assert!(is_excluded(Path::new("/p/.git/subtree-cache/x")));
        // A branch switch is only visible through these.
        assert!(!is_excluded(Path::new("/p/.git/HEAD")));
        assert!(!is_excluded(Path::new("/p/.git/index")));
    }

    #[test]
    fn dedupe_drops_roots_covered_by_a_parent() {
        let roots = vec![
            PathBuf::from("/a/b/c"),
            PathBuf::from("/a"),
            PathBuf::from("/a/b"),
            PathBuf::from("/d"),
        ];
        assert_eq!(
            dedupe_roots(&roots),
            vec![PathBuf::from("/a"), PathBuf::from("/d")]
        );
    }

    #[test]
    fn dedupe_keeps_siblings_with_a_shared_prefix() {
        // `/a/bc` is not inside `/a/b`; a naive string prefix test would drop it.
        let roots = vec![PathBuf::from("/a/b"), PathBuf::from("/a/bc")];
        let deduped = dedupe_roots(&roots);
        assert_eq!(deduped.len(), 2);
        assert!(deduped.contains(&PathBuf::from("/a/b")));
        assert!(deduped.contains(&PathBuf::from("/a/bc")));
    }

    #[test]
    fn dedupe_drops_empty_roots_and_exact_duplicates() {
        let roots = vec![
            PathBuf::from(""),
            PathBuf::from("/a"),
            PathBuf::from("/a"),
        ];
        assert_eq!(dedupe_roots(&roots), vec![PathBuf::from("/a")]);
    }

    #[test]
    fn coalescer_merges_repeated_changes_to_one_event() {
        let mut c = Coalescer::new();
        for _ in 0..10 {
            c.push(FileChangeEvent {
                kind: FileChangeKind::Change,
                path: "/p/a.ts".into(),
            });
        }
        let batch = c.drain_batch();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].kind, FileChangeKind::Change);
    }

    #[test]
    fn coalescer_lets_a_later_kind_supersede_an_earlier_one() {
        let mut c = Coalescer::new();
        c.push(FileChangeEvent {
            kind: FileChangeKind::Add,
            path: "/p/a.ts".into(),
        });
        c.push(FileChangeEvent {
            kind: FileChangeKind::Unlink,
            path: "/p/a.ts".into(),
        });
        let batch = c.drain_batch();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].kind, FileChangeKind::Unlink);
    }

    #[test]
    fn coalescer_preserves_first_seen_order() {
        let mut c = Coalescer::new();
        for name in ["z", "a", "m"] {
            c.push(FileChangeEvent {
                kind: FileChangeKind::Change,
                path: format!("/p/{name}"),
            });
        }
        // Touch the first one again: it must keep its original position.
        c.push(FileChangeEvent {
            kind: FileChangeKind::Change,
            path: "/p/z".into(),
        });
        let batch = c.drain_batch();
        let paths: Vec<&str> = batch.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["/p/z", "/p/a", "/p/m"]);
    }

    #[test]
    fn batches_are_capped_and_the_remainder_survives() {
        let mut c = Coalescer::new();
        for i in 0..(MAX_BATCH + 25) {
            c.push(FileChangeEvent {
                kind: FileChangeKind::Change,
                path: format!("/p/{i}.ts"),
            });
        }
        let first = c.drain_batch();
        assert_eq!(first.len(), MAX_BATCH);
        // The overflow is deferred, not discarded.
        let second = c.drain_batch();
        assert_eq!(second.len(), 25);
        assert!(c.is_empty());
    }

    #[test]
    fn buffer_is_bounded_but_still_updates_known_paths() {
        let mut c = Coalescer::new();
        for i in 0..MAX_BUFFERED {
            c.push(FileChangeEvent {
                kind: FileChangeKind::Change,
                path: format!("/p/{i}.ts"),
            });
        }
        // A brand-new path past the cap is dropped...
        c.push(FileChangeEvent {
            kind: FileChangeKind::Change,
            path: "/p/overflow.ts".into(),
        });
        assert_eq!(c.pending.len(), MAX_BUFFERED);
        assert!(!c.pending.contains_key("/p/overflow.ts"));
        // ...but an already-tracked one still takes its newer kind, so a delete
        // cannot be masked by a full buffer.
        c.push(FileChangeEvent {
            kind: FileChangeKind::Unlink,
            path: "/p/0.ts".into(),
        });
        assert_eq!(c.pending.get("/p/0.ts").map(|slot| slot.0), Some(FileChangeKind::Unlink));
    }

    #[test]
    fn classify_maps_notify_kinds_to_the_renderer_contract() {
        use notify::event::{CreateKind, ModifyKind, RemoveKind};
        assert_eq!(
            classify(&EventKind::Create(CreateKind::File)),
            FileChangeKind::Add
        );
        assert_eq!(
            classify(&EventKind::Remove(RemoveKind::File)),
            FileChangeKind::Unlink
        );
        assert_eq!(
            classify(&EventKind::Modify(ModifyKind::Any)),
            FileChangeKind::Change
        );
        assert_eq!(classify(&EventKind::Any), FileChangeKind::Change);
    }
}
