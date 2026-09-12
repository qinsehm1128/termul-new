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
/// **Takes a path relative to a watched root**, never an absolute one. The
/// segment names below are ordinary directory names, so matching them against a
/// full path lets a directory *above* the root decide: a project living in
/// `~/build/app` would have every one of its events dropped because an ancestor
/// happens to be called `build`. Silently, with the watcher reporting success.
/// `files.watcherExclude` in VSCode is relative to the workspace folder for the
/// same reason. Use [`is_excluded_within`] unless the path is already relative.
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

/// Roots in the form events actually arrive in.
///
/// FSEvents reports real paths, and on macOS even `/tmp` is a symlink — so a
/// symlinked project root would never prefix-match its own events. The filter
/// would then fall back to judging the whole absolute path, bringing back the
/// exact ancestor-name bug it exists to prevent, for precisely that root.
///
/// A root that cannot be resolved (it does not exist yet) is kept as given;
/// nothing is watching it either way.
fn resolve_filter_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .map(|root| root.canonicalize().unwrap_or_else(|_| root.clone()))
        .collect()
}

/// Whether an absolute event path is excluded, judged inside its own root.
///
/// `dedupe_roots` guarantees no root contains another, so at most one root can
/// match. A path under none of them cannot be judged relatively; falling back to
/// the whole path is the conservative choice, and it is unreachable in practice
/// because every event comes from a registered root.
fn is_excluded_within(path: &Path, roots: &[PathBuf]) -> bool {
    for root in roots {
        if let Ok(relative) = path.strip_prefix(root) {
            return is_excluded(relative);
        }
    }
    is_excluded(path)
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

/// The kind for one path of an event, resolving a rename into the unlink/add
/// pair the renderer contract is built around.
///
/// A rename used to arrive as `change` on both ends. The renderer only closes an
/// editor tab on `unlink` (`use-file-watcher.ts`), so renaming an open file left
/// a tab pointing at a path that no longer exists, and the new name never
/// registered as an addition.
///
/// `From`/`To`/`Both` say which end a path is. macOS reports each side
/// separately as `Any`, with nothing in the event to tell them apart — so ask
/// the filesystem: the side that is gone is the one that went away. That stat
/// only runs on rename events, which are rare next to writes.
fn kind_for_path(event_kind: &EventKind, path: &Path, index: usize, total: usize) -> FileChangeKind {
    use notify::event::{ModifyKind, RenameMode};
    let EventKind::Modify(ModifyKind::Name(mode)) = event_kind else {
        return classify(event_kind);
    };
    match mode {
        RenameMode::From => FileChangeKind::Unlink,
        RenameMode::To => FileChangeKind::Add,
        RenameMode::Both if total == 2 => {
            if index == 0 {
                FileChangeKind::Unlink
            } else {
                FileChangeKind::Add
            }
        }
        _ => {
            if path.symlink_metadata().is_ok() {
                FileChangeKind::Add
            } else {
                FileChangeKind::Unlink
            }
        }
    }
}

/// Turn one raw notify event into the changes worth delivering.
///
/// One output per surviving path, not one per event. A rename arrives as a
/// single event carrying both the old and the new path; taking only the first
/// silently lost the other end of it.
fn expand_event(event: notify::Event, roots: &[PathBuf]) -> Vec<FileChangeEvent> {
    let total = event.paths.len();
    event
        .paths
        .into_iter()
        .enumerate()
        .filter(|(_, path)| !is_excluded_within(path, roots))
        .filter_map(|(index, path)| {
            let kind = kind_for_path(&event.kind, &path, index, total);
            path.to_str().map(|path| FileChangeEvent {
                kind,
                path: path.replace('\\', "/"),
            })
        })
        .collect()
}

/// One merged batch ready for the renderer.
type Batch = Vec<FileChangeEvent>;

struct Coalescer {
    /// Kind and queue position per path.
    pending: HashMap<String, (FileChangeKind, u64)>,
    /// Queue position -> path, so taking the oldest `MAX_BATCH` is a walk from
    /// the front rather than a sort of everything still waiting.
    ///
    /// The pair used to be a single `HashMap` sorted on every flush. At the
    /// 30 000-event cap that meant cloning and sorting ~30 000 strings per
    /// 500-event batch, 60 batches deep — and the coalescer does not read its
    /// input channel while it drains, so the slower it drained the more the
    /// upstream queue grew.
    queue: std::collections::BTreeMap<u64, String>,
    order: u64,
    overflow_reported: bool,
}

impl Coalescer {
    fn new() -> Self {
        Self {
            pending: HashMap::new(),
            queue: std::collections::BTreeMap::new(),
            order: 0,
            overflow_reported: false,
        }
    }

    fn push(&mut self, event: FileChangeEvent) {
        // A later event supersedes an earlier one for the same path but keeps
        // its original position, so "created then modified" still reads as one
        // change at the point the file first appeared.
        if let Some(slot) = self.pending.get_mut(&event.path) {
            slot.0 = event.kind;
            return;
        }

        if self.pending.len() >= MAX_BUFFERED {
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
        self.queue.insert(order, event.path.clone());
        self.pending.insert(event.path, (event.kind, order));
    }

    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Take up to `MAX_BATCH` events, oldest first. Anything beyond the cap
    /// stays pending for the next flush rather than being dropped.
    fn drain_batch(&mut self) -> Batch {
        let mut batch = Batch::with_capacity(MAX_BATCH.min(self.pending.len()));
        while batch.len() < MAX_BATCH {
            let Some((_, path)) = self.queue.pop_first() else {
                break;
            };
            if let Some((kind, _)) = self.pending.remove(&path) {
                batch.push(FileChangeEvent { kind, path });
            }
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

                    let Some(service) = weak.upgrade() else { break };
                    let sink = service.lock_inner().sink.clone();
                    let Some(sink) = sink else {
                        // Nobody is listening yet. Hold the batch rather than
                        // draining it into the void: roots can be set before the
                        // renderer subscribes, and a webview reload leaves a
                        // window with no sink — changes made in either window
                        // used to vanish with no trace. `MAX_BUFFERED` still
                        // bounds how long this can go on.
                        deadline = Some(Instant::now() + COALESCE_WINDOW);
                        continue;
                    };

                    let batch = coalescer.drain_batch();
                    deadline = if coalescer.is_empty() {
                        None
                    } else {
                        // More than one batch worth was pending; come straight
                        // back for the rest instead of waiting out another
                        // quiet window.
                        Some(Instant::now())
                    };

                    if let Err(err) = sink.send(batch) {
                        log::warn!("[fs-watcher] failed to deliver batch: {err}");
                    }
                }
            })
            .expect("spawn fs-watcher coalescer thread");

        service
    }

    /// Take the state lock, recovering from a poisoned mutex.
    ///
    /// `Inner` is a watcher handle, a root list and a channel — a panic while
    /// holding this lock cannot leave any of them half-written. The three call
    /// sites used to disagree about poisoning (one returned an error, one
    /// silently dropped every batch, one silently failed to install the sink),
    /// so an unrelated panic anywhere turned into file watching that was dead
    /// for the rest of the session with nothing to point at.
    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_sink(&self, channel: Channel<Batch>) {
        self.lock_inner().sink = Some(channel);
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

        let mut inner = self.lock_inner();

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
        // The filter needs the roots so it can judge each path *inside* its own
        // root. A fresh watcher is built for every root change, so the set the
        // closure captures is always the one it is watching.
        //
        // Watching still uses the path as given; only the filter is resolved.
        let filter_roots = resolve_filter_roots(&desired);
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else { return };
                for change in expand_event(event, &filter_roots) {
                    let _ = tx.send(change);
                }
            },
            Config::default(),
        )
        .map_err(|err| format!("failed to create fs watcher: {err}"))?;

        // A root that cannot be watched must not take the others down with it.
        // One stale path in a group of twenty projects used to fail the whole
        // call, leaving nothing watched — and the failure path dropped this
        // half-registered watcher inline, joining its backend thread while
        // still holding the state lock.
        let mut failures: Vec<String> = Vec::new();
        let mut watched: Vec<PathBuf> = Vec::new();
        for root in &desired {
            match watcher.watch(root, RecursiveMode::Recursive) {
                Ok(()) => watched.push(root.clone()),
                Err(err) => failures.push(format!("{}: {err}", root.display())),
            }
        }

        if watched.is_empty() {
            drop(inner);
            release_watcher(Some(watcher));
            return Err(format!("failed to watch any root ({})", failures.join("; ")));
        }
        if !failures.is_empty() {
            log::warn!(
                "[fs-watcher] watching {}/{} roots; skipped {}",
                watched.len(),
                desired.len(),
                failures.join("; ")
            );
        }

        let previous = inner.watcher.replace(watcher);
        // What is actually watched, not what was asked for. Caching the request
        // would let the `inner.roots == desired` short-circuit above turn a root
        // that was merely missing at this moment into one that is never watched
        // again: the next identical request would be answered from cache and
        // never retry it.
        inner.roots = watched;
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

    fn event(kind: EventKind, paths: &[&str]) -> notify::Event {
        notify::Event {
            kind,
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    #[test]
    fn expands_one_change_per_path_so_a_rename_keeps_both_ends() {
        use notify::event::{ModifyKind, RenameMode};
        let roots = vec![PathBuf::from("/p")];
        let changes = expand_event(
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &["/p/old.ts", "/p/new.ts"],
            ),
            &roots,
        );
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, "/p/old.ts");
        assert_eq!(changes[1].path, "/p/new.ts");
    }

    #[test]
    fn expansion_drops_excluded_paths_but_keeps_their_siblings() {
        use notify::event::ModifyKind;
        let roots = vec![PathBuf::from("/p")];
        let changes = expand_event(
            event(
                EventKind::Modify(ModifyKind::Any),
                &["/p/node_modules/x.js", "/p/src/main.rs"],
            ),
            &roots,
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "/p/src/main.rs");
    }

    #[test]
    fn exclusion_is_judged_inside_the_root_not_above_it() {
        use notify::event::ModifyKind;
        // The root itself is called `build`. Judged against the whole path that
        // name alone silenced every event the project could ever produce — the
        // watcher registering fine and then never firing.
        let roots = vec![PathBuf::from("/Users/me/build/app")];
        let changes = expand_event(
            event(
                EventKind::Modify(ModifyKind::Any),
                &["/Users/me/build/app/src/main.rs"],
            ),
            &roots,
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "/Users/me/build/app/src/main.rs");

        // A segment of the same name *below* the root is still excluded.
        let inside = expand_event(
            event(
                EventKind::Modify(ModifyKind::Any),
                &["/Users/me/build/app/build/out.js"],
            ),
            &roots,
        );
        assert!(inside.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn filter_roots_are_resolved_so_a_symlinked_root_matches_its_own_events() {
        let dir = tempfile::tempdir().expect("temp dir");
        // Deliberately named after an excluded segment: that is what makes the
        // difference between the two forms visible rather than theoretical.
        let real = dir.path().join("build");
        std::fs::create_dir(&real).expect("create dir");
        let link = dir.path().join("link-to-build");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        // The caller asks for the link; events arrive under the resolved path.
        let resolved = resolve_filter_roots(&[link.clone()]);
        assert_eq!(resolved, vec![real.canonicalize().expect("canonicalize")]);

        let event_path = real.canonicalize().expect("canonicalize").join("src/main.rs");
        // Resolved: judged inside the root, so the root's own name is not a
        // segment that can exclude it.
        assert!(!is_excluded_within(&event_path, &resolved));
        // Unresolved: the prefix never matches, the whole absolute path gets
        // judged, and the root's own `build` segment silences the project.
        assert!(is_excluded_within(&event_path, &[link]));
    }

    #[test]
    fn exclusion_uses_the_root_the_path_belongs_to() {
        use notify::event::ModifyKind;
        let roots = vec![PathBuf::from("/a/dist"), PathBuf::from("/b")];
        // `/a/dist` is a root: its own name must not exclude its contents.
        let from_a = expand_event(
            event(EventKind::Modify(ModifyKind::Any), &["/a/dist/src/x.ts"]),
            &roots,
        );
        assert_eq!(from_a.len(), 1);
        // `/b/dist` is not a root, just a build directory inside one.
        let from_b = expand_event(
            event(EventKind::Modify(ModifyKind::Any), &["/b/dist/bundle.js"]),
            &roots,
        );
        assert!(from_b.is_empty());
    }

    #[test]
    fn drain_keeps_first_seen_order_across_batch_boundaries() {
        let mut c = Coalescer::new();
        for i in 0..(MAX_BATCH + 3) {
            c.push(FileChangeEvent {
                kind: FileChangeKind::Change,
                path: format!("/p/{i}.ts"),
            });
        }
        let first = c.drain_batch();
        assert_eq!(first[0].path, "/p/0.ts");
        assert_eq!(first[MAX_BATCH - 1].path, format!("/p/{}.ts", MAX_BATCH - 1));
        let second = c.drain_batch();
        let tail: Vec<String> = second.iter().map(|e| e.path.clone()).collect();
        assert_eq!(
            tail,
            vec![
                format!("/p/{}.ts", MAX_BATCH),
                format!("/p/{}.ts", MAX_BATCH + 1),
                format!("/p/{}.ts", MAX_BATCH + 2),
            ]
        );
    }

    /// Measures the real watch surface of this repository.
    ///
    /// `#[ignore]` because it walks the whole working tree and its numbers
    /// depend on local build state; run it explicitly with
    /// `cargo test --lib fs_watcher::tests::measure -- --ignored --nocapture`.
    /// It exists because the alignment decision rested on an untested premise —
    /// that recursive watching is affordable once excludes are applied — and a
    /// premise like that deserves a number rather than an argument.
    #[test]
    #[ignore]
    fn measure_exclusion_ratio_on_this_repository() {
        /// Counts entries. `prune` mirrors the watcher: an excluded directory
        /// is counted once and never descended into.
        fn walk(dir: &Path, root: &Path, prune: bool, seen: &mut u64, depth: usize) {
            if depth > 12 {
                return;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                *seen += 1;
                // Judged relative to the root, exactly as the watcher does — so
                // the number stays honest even when the repo lives under a
                // directory whose name is on the exclude list.
                if prune && is_excluded_within(&path, std::slice::from_ref(&root.to_path_buf())) {
                    // Not descending is where the saving comes from — the
                    // directory costs one entry instead of its whole subtree.
                    continue;
                }
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    walk(&path, root, prune, seen, depth + 1);
                }
            }
        }

        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .to_path_buf();

        let mut watched = 0u64;
        walk(&root, &root, true, &mut watched, 0);
        let mut unfiltered = 0u64;
        walk(&root, &root, false, &mut unfiltered, 0);

        println!("root={}", root.display());
        println!("entries without exclude = {unfiltered}");
        println!("entries with exclude    = {watched}");
        println!(
            "watch surface removed   = {:.1}%  ({} entries)",
            ((unfiltered - watched) as f64 / unfiltered.max(1) as f64) * 100.0,
            unfiltered - watched
        );
        assert!(unfiltered > 0, "walk found nothing — wrong root?");
        assert!(
            watched < unfiltered,
            "exclusion removed nothing; the gate is not doing its job"
        );
    }

    #[test]
    fn a_rename_becomes_an_unlink_and_an_add_not_two_changes() {
        use notify::event::{ModifyKind, RenameMode};
        let roots = vec![PathBuf::from("/p")];
        let changes = expand_event(
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &["/p/old.ts", "/p/new.ts"],
            ),
            &roots,
        );
        // Both ends used to arrive as `change`, and the renderer only closes an
        // editor tab on `unlink` — so the old name kept a tab pointing at a path
        // that no longer existed.
        assert_eq!(changes[0].kind, FileChangeKind::Unlink);
        assert_eq!(changes[1].kind, FileChangeKind::Add);
    }

    #[test]
    fn one_sided_rename_modes_pick_their_own_side() {
        use notify::event::{ModifyKind, RenameMode};
        let roots = vec![PathBuf::from("/p")];
        let from = expand_event(
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                &["/p/old.ts"],
            ),
            &roots,
        );
        assert_eq!(from[0].kind, FileChangeKind::Unlink);
        let to = expand_event(
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                &["/p/new.ts"],
            ),
            &roots,
        );
        assert_eq!(to[0].kind, FileChangeKind::Add);
    }

    #[test]
    fn an_ambiguous_rename_asks_the_filesystem_which_side_it_is() {
        use notify::event::{ModifyKind, RenameMode};
        // macOS reports each side as `Any` with nothing to tell them apart.
        let existing = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let roots = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))];

        let present = expand_event(
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
                &[existing.to_str().expect("utf-8 path")],
            ),
            &roots,
        );
        assert_eq!(present[0].kind, FileChangeKind::Add);

        let gone_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("definitely-not-here-xyz");
        let gone = expand_event(
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
                &[gone_path.to_str().expect("utf-8 path")],
            ),
            &roots,
        );
        assert_eq!(gone[0].kind, FileChangeKind::Unlink);
    }

    #[test]
    fn an_unwatchable_root_does_not_take_the_others_down_with_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let service = FsWatcherService::new();

        // One live project plus one whose directory is gone — the shape a group
        // takes as soon as a member is moved or deleted. (A sibling path, not a
        // child: `dedupe_roots` would fold a child into its parent and the test
        // would prove nothing.) Failing the whole call left every remaining
        // project unwatched.
        let missing = PathBuf::from("/se-manager-missing-root-xyz");
        let result = service.set_roots(vec![dir.path().to_path_buf(), missing.clone()]);
        assert!(
            result.is_ok(),
            "one bad root must not fail the set: {result:?}"
        );
        // Only what is actually watched is cached. Caching the request instead
        // would make the `inner.roots == desired` short-circuit answer the next
        // identical call from cache, so a root that was merely missing for a
        // moment would never be watched again.
        assert_eq!(service.lock_inner().roots, vec![dir.path().to_path_buf()]);

        // The same request again must therefore retry the missing root rather
        // than short-circuit.
        let retried = service.set_roots(vec![dir.path().to_path_buf(), missing]);
        assert!(retried.is_ok());
        assert_eq!(service.lock_inner().roots, vec![dir.path().to_path_buf()]);

        // Nothing watchable at all is still an error — that one the caller needs.
        let all_bad = service.set_roots(vec![PathBuf::from("/se-manager-missing-root-2-xyz")]);
        assert!(all_bad.is_err());
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
