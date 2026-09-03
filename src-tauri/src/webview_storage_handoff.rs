//! WebView storage across a bundle-identifier rename (M-05).
//!
//! # Three platforms, three different answers
//!
//! [`crate::legacy_appdata`] carries the whole `app_data_dir` tree forward when
//! the bundle identifier changes. Whether the WebView's own storage rides along
//! with it depends entirely on where the runtime put that storage, and the three
//! answers do not generalise:
//!
//! - **Linux** — nothing to do. tauri sets the webview's `data_directory` from
//!   `resolve(identifier, LocalData)` under a `linux | windows` cfg, and wry
//!   uses it as the WebKitGTK `base_data_directory`. On Linux `LocalData` and
//!   `Data` are the same XDG root, so that directory *is* inside `app_data_dir`
//!   and the existing carry-forward already moves it. Adding a second copier
//!   here would only race the first.
//!
//! - **Windows** — one extra copy. WebView2 puts `EBWebView` under
//!   `%LOCALAPPDATA%\{identifier}`, while `app_data_dir` is Roaming
//!   (`%APPDATA%\{identifier}`). The carry-forward never reaches it, so this
//!   module runs the same copy over that one directory — literally the same
//!   function, so "copy only, never overwrite, never delete, skip symlinks"
//!   (FORBID-05) holds by construction rather than by a second implementation
//!   agreeing to behave.
//!
//! - **macOS** — directory migration is impossible. That `data_directory` cfg
//!   does not include macOS, so it stays `None` and WKWebView falls back to
//!   `~/Library/WebKit/{bundle_id}`: storage partitioned *by bundle identifier*,
//!   outside `app_data_dir`, and not relocatable by moving files. The only thing
//!   left is to replay the app's own keys explicitly — see below.
//!
//! # The macOS handoff, and why it is two legs
//!
//! The old store can only be read by a process running under the old
//! identifier, and the new store can only be written by one running under the
//! new identifier. No single boot is both. So:
//!
//! - **Write leg** ([`capture`]), under the *old* identifier: the renderer hands
//!   over its `localStorage`, this module keeps only the app-owned keys and
//!   writes them to a handoff file inside `app_data_dir` — the tree
//!   [`crate::legacy_appdata`] already carries across the rename.
//! - **Read leg** ([`pending`] then [`mark_consumed`]), under the *new*
//!   identifier: the file arrives with the rest of `app_data_dir`, the renderer
//!   reinstates the keys, and only then is the handoff marked consumed. Marking
//!   on success rather than on read is deliberate: a boot that fails halfway has
//!   nothing fresher to protect and should retry, while a boot that succeeded
//!   must never replay stale values over what the user has changed since.
//!
//! Only keys carrying this app's own prefixes are eligible
//! ([`is_app_owned_key`]). Third-party site data in the same WKWebView is not
//! ours to move, and the filter runs here rather than in the renderer so
//! over-collection on the caller's side cannot widen it.
//!
//! Every failure in this module is non-fatal. A missing, unreadable, or
//! wrong-schema handoff logs and returns "nothing to replay"; the renderer's
//! read path accepts the legacy prefixes as well as the canonical ones, so the
//! cost of a failed replay is that a value is read from its old key, not that it
//! is lost.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::brand::{self, BrandCanonical};
use crate::legacy_appdata::{self, CarryForwardReport};

/// WebView2's per-identifier data directory under `%LOCALAPPDATA%\{id}`.
pub const WEBVIEW_DATA_DIR: &str = "EBWebView";

/// Handoff file name, relative to `app_data_dir`. Carries no brand string: the
/// directory it sits in is already identifier-scoped.
pub const HANDOFF_FILE: &str = "webview-storage-handoff.json";

/// On-disk schema of [`StorageHandoff`]. A file written by a newer schema is
/// left alone rather than half-understood.
pub const HANDOFF_SCHEMA_VERSION: u32 = 1;

/// Which of the three answers above applies to a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Linux,
    MacOs,
    Windows,
}

impl HostPlatform {
    /// The platform this binary was built for.
    ///
    /// Anything that is neither Windows nor Apple takes the Linux branch: what
    /// that branch actually asserts is "the webview's data directory is inside
    /// `app_data_dir`", which is true of every WebKitGTK host, not of Linux
    /// specifically.
    #[must_use]
    pub fn host() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// What a host has to do to keep WebView storage across the rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebViewStorageStrategy {
    /// The store is inside `app_data_dir`; the existing carry-forward moves it.
    RidesAlongWithAppData,
    /// The store is outside `app_data_dir`; copy that one directory too.
    CopyLocalWebViewDir,
    /// The store cannot be moved; replay the app's own keys through a handoff.
    ReplayAppOwnedKeys,
}

#[must_use]
pub fn strategy_for(platform: HostPlatform) -> WebViewStorageStrategy {
    match platform {
        HostPlatform::Linux => WebViewStorageStrategy::RidesAlongWithAppData,
        HostPlatform::Windows => WebViewStorageStrategy::CopyLocalWebViewDir,
        HostPlatform::MacOs => WebViewStorageStrategy::ReplayAppOwnedKeys,
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Copy the pre-rename `EBWebView` directory beside `app_local_data_dir` into
/// the canonical one.
///
/// `app_local_data_dir` is `%LOCALAPPDATA%\{identifier}` — the same
/// `<base>/<bundle id>` shape [`legacy_appdata::matching_legacy_root`] expects,
/// so the prod/dev pairing and the "no rename has happened yet" case are decided
/// there rather than re-derived here. The copy itself is
/// [`legacy_appdata::carry_forward`], unchanged, so FORBID-05 needs no second
/// proof.
///
/// Reads the brand seam, so it must be called on the thread that owns it
/// (FORBID-07).
pub fn carry_forward_local_webview_data(
    app_local_data_dir: &Path,
) -> io::Result<CarryForwardReport> {
    let Some(legacy_root) = legacy_appdata::matching_legacy_root(app_local_data_dir) else {
        return Ok(CarryForwardReport::default());
    };
    legacy_appdata::carry_forward(
        &legacy_root.join(WEBVIEW_DATA_DIR),
        &app_local_data_dir.join(WEBVIEW_DATA_DIR),
    )
}

// ---------------------------------------------------------------------------
// macOS — app-owned key replay
// ---------------------------------------------------------------------------

/// The app-owned `localStorage` keys captured under one identifier, waiting to
/// be reinstated under another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageHandoff {
    pub schema_version: u32,
    /// Bundle identifier whose WebView store these came out of. Diagnostic, and
    /// the answer to "is this file describing the rename I am in the middle
    /// of?" when one shows up unexpectedly.
    pub written_under: String,
    /// Set by [`mark_consumed`] once a replay has succeeded. Its presence is
    /// what stops a second boot from writing these values back over whatever
    /// the user has changed since.
    #[serde(default)]
    pub consumed_under: Option<String>,
    pub entries: BTreeMap<String, String>,
}

/// Which leg of the handoff a process running under `current_bundle_id` is on.
///
/// The old identifier is a fixed, permanent value ([`brand::LEGACY`]), so this
/// needs no seam read: a build still shipping under it can only capture, and
/// anything else can only replay. A fresh install under the new identifier takes
/// the replay branch and finds nothing, which is the correct no-op.
#[must_use]
pub fn leg_for(current_bundle_id: &str) -> HandoffLeg {
    if current_bundle_id == brand::LEGACY.bundle_id
        || current_bundle_id == brand::LEGACY.bundle_id_dev
    {
        HandoffLeg::Capture
    } else {
        HandoffLeg::Replay
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffLeg {
    /// Still under the pre-rename identifier: read the store out.
    Capture,
    /// Under the post-rename identifier: put the store back.
    Replay,
}

#[must_use]
pub fn handoff_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(HANDOFF_FILE)
}

/// Whether `key` is one this app wrote.
///
/// Both spellings of both prefixes are eligible. The capture runs under the old
/// identifier, which may or may not be a build that has already flipped the
/// prefixes (T-A08), and a key missed here is a key that cannot be recovered
/// afterwards — the old store stops being readable the moment the identifier
/// changes.
///
/// `canonical` is passed in rather than read here so callers resolve the seam on
/// their own thread (FORBID-07).
#[must_use]
pub fn is_app_owned_key(key: &str, canonical: &BrandCanonical) -> bool {
    [
        brand::LEGACY.storage_prefix,
        brand::LEGACY.storage_key_prefix,
        canonical.storage_prefix,
        canonical.storage_key_prefix,
    ]
    .iter()
    .any(|prefix| key.starts_with(prefix))
}

/// The app-owned subset of `entries`, in key order.
#[must_use]
pub fn select_app_owned<I>(entries: I, canonical: &BrandCanonical) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    entries
        .into_iter()
        .filter(|(key, _)| is_app_owned_key(key, canonical))
        .collect()
}

/// Write leg. Records the app-owned keys of `entries` under `app_data_dir`.
///
/// Overwrites any earlier handoff: while the app is still running under the old
/// identifier each boot's values are fresher than the last, and nothing has read
/// them yet.
pub fn capture<I>(
    app_data_dir: &Path,
    written_under: &str,
    entries: I,
    canonical: &BrandCanonical,
) -> io::Result<StorageHandoff>
where
    I: IntoIterator<Item = (String, String)>,
{
    let handoff = StorageHandoff {
        schema_version: HANDOFF_SCHEMA_VERSION,
        written_under: written_under.to_string(),
        consumed_under: None,
        entries: select_app_owned(entries, canonical),
    };
    write_handoff(&handoff_path(app_data_dir), &handoff)?;
    Ok(handoff)
}

/// Read leg, first half. The handoff waiting to be replayed, if there is one.
///
/// `None` covers every reason there is nothing to do — absent, unreadable,
/// unparseable, a schema this build does not know, or already consumed — because
/// none of them is recoverable and all of them degrade the same way: the
/// renderer reads the legacy key instead.
#[must_use]
pub fn pending(app_data_dir: &Path) -> Option<StorageHandoff> {
    let path = handoff_path(app_data_dir);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!(
                "[webview-storage] handoff {} is unreadable: {error}. App-owned keys stay on their pre-rename spelling.",
                path.display()
            );
            return None;
        }
    };

    let handoff: StorageHandoff = match serde_json::from_slice(&bytes) {
        Ok(handoff) => handoff,
        Err(error) => {
            log::warn!(
                "[webview-storage] handoff {} does not parse: {error}. App-owned keys stay on their pre-rename spelling.",
                path.display()
            );
            return None;
        }
    };

    if handoff.schema_version != HANDOFF_SCHEMA_VERSION {
        log::warn!(
            "[webview-storage] handoff {} is schema {} but this build reads {HANDOFF_SCHEMA_VERSION}; leaving it alone",
            path.display(),
            handoff.schema_version
        );
        return None;
    }
    if handoff.consumed_under.is_some() {
        return None;
    }
    Some(handoff)
}

/// Read leg, second half. Marks the handoff consumed so it is replayed once.
///
/// Called *after* the renderer has reinstated the keys, not when they were
/// handed out: a replay that never landed should be retried, and only a replay
/// that did land can be clobbered by a later one. Idempotent.
pub fn mark_consumed(app_data_dir: &Path, consumed_under: &str) -> io::Result<()> {
    let path = handoff_path(app_data_dir);
    let bytes = fs::read(&path)?;
    let mut handoff: StorageHandoff = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if handoff.consumed_under.is_some() {
        return Ok(());
    }
    handoff.consumed_under = Some(consumed_under.to_string());
    write_handoff(&path, &handoff)
}

/// Serialize to a sibling temp file and rename over the target, so a crash
/// mid-write leaves the previous handoff intact rather than a truncated one that
/// [`pending`] would discard.
fn write_handoff(path: &Path, handoff: &StorageHandoff) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_vec_pretty(handoff)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, &serialized)?;
    fs::rename(&temporary, path)
}

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

/// The one branch that needs no renderer: the Windows `EBWebView` copy.
///
/// Runs on the setup thread — `brand::canonical()` is thread-local (FORBID-07).
/// Non-fatal: a failure costs a WebView cache, not user data.
pub fn run_at_startup<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if strategy_for(HostPlatform::host()) != WebViewStorageStrategy::CopyLocalWebViewDir {
        return;
    }
    let local_data_dir = match app.path().app_local_data_dir() {
        Ok(dir) => dir,
        Err(error) => {
            log::warn!("[webview-storage] no local app data directory: {error}");
            return;
        }
    };
    match carry_forward_local_webview_data(&local_data_dir) {
        Ok(report) if report.is_noop() => {}
        Ok(report) => log::info!(
            "[webview-storage] carried {} WebView2 file(s) forward ({} already present, {} link(s) skipped)",
            report.copied,
            report.already_present,
            report.skipped_links
        ),
        Err(error) => log::warn!(
            "[webview-storage] WebView2 carry-forward failed: {error}. Site data under the pre-rename identifier stays there."
        ),
    }
}

/// Write leg, as invoked by the renderer under the old identifier.
///
/// Takes the renderer's whole `localStorage`; the app-owned filter runs here.
#[tauri::command]
pub fn webview_storage_handoff_capture(
    app: tauri::AppHandle,
    entries: BTreeMap<String, String>,
) -> Result<usize, String> {
    // Resolved on this thread, then handed to the pure function (FORBID-07).
    let canonical = brand::canonical();
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let bundle_id = app.config().identifier.clone();
    capture(&app_data_dir, &bundle_id, entries, &canonical)
        .map(|handoff| handoff.entries.len())
        .map_err(|error| error.to_string())
}

/// Read leg, first half: the keys the renderer should reinstate, if any.
#[tauri::command]
pub fn webview_storage_handoff_pending(
    app: tauri::AppHandle,
) -> Result<Option<BTreeMap<String, String>>, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    Ok(pending(&app_data_dir).map(|handoff| handoff.entries))
}

/// Read leg, second half: called once the renderer has written them back.
#[tauri::command]
pub fn webview_storage_handoff_mark_consumed(app: tauri::AppHandle) -> Result<(), String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let bundle_id = app.config().identifier.clone();
    mark_consumed(&app_data_dir, &bundle_id).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The post-rename brand, as Wave 5 will leave it.
    fn post_rename() -> BrandCanonical {
        BrandCanonical {
            bundle_id: "com.se-manager.app",
            bundle_id_dev: "com.se-manager.app.dev",
            storage_prefix: "se-store:",
            storage_key_prefix: "se:",
            ..brand::DEFAULT_CANONICAL
        }
    }

    // -----------------------------------------------------------------------
    // All three branches, on whatever host is running the suite
    // -----------------------------------------------------------------------

    #[test]
    fn every_platform_gets_the_strategy_its_webview_layout_requires() {
        assert_eq!(
            strategy_for(HostPlatform::Linux),
            WebViewStorageStrategy::RidesAlongWithAppData,
            "on Linux the webview data directory is inside app_data_dir; a second copier here \
             would duplicate legacy_appdata's work"
        );
        assert_eq!(
            strategy_for(HostPlatform::Windows),
            WebViewStorageStrategy::CopyLocalWebViewDir,
            "EBWebView sits under %LOCALAPPDATA%, outside the Roaming app_data_dir the \
             carry-forward reaches"
        );
        assert_eq!(
            strategy_for(HostPlatform::MacOs),
            WebViewStorageStrategy::ReplayAppOwnedKeys,
            "WKWebView storage is partitioned by bundle id and cannot be moved by copying files"
        );
    }

    // -----------------------------------------------------------------------
    // Linux — the keys survive because app_data_dir moved
    // -----------------------------------------------------------------------

    /// The Linux claim is not "we do nothing", it is "doing nothing is enough".
    /// So the assertion is on the outcome: with the webview store where Linux
    /// puts it — inside `app_data_dir` — the app-owned keys are readable under
    /// the canonical identifier after the existing carry-forward has run, with
    /// no call into this module at all.
    #[test]
    fn linux_keys_are_readable_after_the_app_data_dir_copy_alone() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        // On Linux LocalData == Data, so `resolve(identifier, LocalData)` and
        // app_data_dir are the same directory.
        let legacy_root = base.join(brand::LEGACY.bundle_id);
        let canonical_root = base.join(post_rename().bundle_id);
        let store = legacy_root.join("localstorage/leveldb");
        fs::create_dir_all(&store).unwrap();
        let payload = format!(
            "{}editor::layout\u{0}{{\"split\":2}}",
            brand::LEGACY.storage_prefix
        );
        fs::write(store.join("000003.log"), payload.as_bytes()).unwrap();

        let _brand = brand::override_canonical(post_rename());
        let legacy = legacy_appdata::matching_legacy_root(&canonical_root)
            .expect("the pre-rename root is the sibling of the canonical one");
        let report = legacy_appdata::carry_forward(&legacy, &canonical_root).unwrap();

        assert_eq!(report.copied, 1);
        let carried = canonical_root.join("localstorage/leveldb/000003.log");
        let bytes = fs::read(&carried).expect("the webview store rode along with app_data_dir");
        assert!(
            String::from_utf8_lossy(&bytes).contains(brand::LEGACY.storage_prefix),
            "the app-owned key must be readable under the canonical identifier"
        );
    }

    // -----------------------------------------------------------------------
    // Windows — EBWebView needs its own pass
    // -----------------------------------------------------------------------

    #[test]
    fn windows_carries_ebwebview_from_the_pre_rename_local_app_data_root() {
        let temp = tempfile::tempdir().unwrap();
        let local_app_data = temp.path();
        let legacy = local_app_data.join(brand::LEGACY.bundle_id);
        let canonical = local_app_data.join(post_rename().bundle_id);
        fs::create_dir_all(legacy.join(format!("{WEBVIEW_DATA_DIR}/Default/Local Storage")))
            .unwrap();
        fs::write(
            legacy.join(format!(
                "{WEBVIEW_DATA_DIR}/Default/Local Storage/leveldb.log"
            )),
            b"webview2-site-data",
        )
        .unwrap();
        fs::create_dir_all(&canonical).unwrap();

        let _brand = brand::override_canonical(post_rename());
        let report = carry_forward_local_webview_data(&canonical).unwrap();

        assert_eq!(report.copied, 1);
        assert_eq!(
            fs::read(canonical.join(format!(
                "{WEBVIEW_DATA_DIR}/Default/Local Storage/leveldb.log"
            )))
            .unwrap(),
            b"webview2-site-data"
        );
        // Copy only: the source is untouched and a second pass changes nothing.
        assert!(legacy
            .join(format!(
                "{WEBVIEW_DATA_DIR}/Default/Local Storage/leveldb.log"
            ))
            .is_file());
        assert!(carry_forward_local_webview_data(&canonical)
            .unwrap()
            .is_noop());
    }

    /// prod and dev are two installs a user may hold at once, with genuinely
    /// different WebView2 contents. A dev build must read the dev tree; carrying
    /// prod's site data into it would be the same cross-channel contamination
    /// `legacy_appdata` pairs identifiers to avoid.
    #[test]
    fn windows_pairs_each_install_channel_with_its_own_pre_rename_root() {
        let temp = tempfile::tempdir().unwrap();
        let local_app_data = temp.path();
        for (root, marker) in [
            (brand::LEGACY.bundle_id, "prod-only"),
            (brand::LEGACY.bundle_id_dev, "dev-only"),
        ] {
            let dir = local_app_data.join(root).join(WEBVIEW_DATA_DIR);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("{marker}.log")), marker.as_bytes()).unwrap();
        }
        let canonical_dev = local_app_data.join(post_rename().bundle_id_dev);
        fs::create_dir_all(&canonical_dev).unwrap();

        let _brand = brand::override_canonical(post_rename());
        let report = carry_forward_local_webview_data(&canonical_dev).unwrap();

        assert_eq!(report.copied, 1);
        assert_eq!(
            fs::read(canonical_dev.join(WEBVIEW_DATA_DIR).join("dev-only.log")).unwrap(),
            b"dev-only"
        );
        assert!(
            !canonical_dev
                .join(WEBVIEW_DATA_DIR)
                .join("prod-only.log")
                .exists(),
            "the release install's WebView data must never land in the dev install"
        );
    }

    #[test]
    fn windows_never_overwrites_webview_data_the_new_install_already_wrote() {
        let temp = tempfile::tempdir().unwrap();
        let local_app_data = temp.path();
        let legacy = local_app_data.join(brand::LEGACY.bundle_id);
        let canonical = local_app_data.join(post_rename().bundle_id);
        fs::create_dir_all(legacy.join(WEBVIEW_DATA_DIR)).unwrap();
        fs::write(legacy.join(WEBVIEW_DATA_DIR).join("cookies"), b"stale").unwrap();
        fs::create_dir_all(canonical.join(WEBVIEW_DATA_DIR)).unwrap();
        fs::write(canonical.join(WEBVIEW_DATA_DIR).join("cookies"), b"fresh").unwrap();

        let _brand = brand::override_canonical(post_rename());
        let report = carry_forward_local_webview_data(&canonical).unwrap();

        assert_eq!(report.copied, 0);
        assert_eq!(report.already_present, 1);
        assert_eq!(
            fs::read(canonical.join(WEBVIEW_DATA_DIR).join("cookies")).unwrap(),
            b"fresh"
        );
    }

    // There is deliberately no "copies nothing before the rename lands" test
    // here. That state is decided three layers down — `matching_legacy_root`
    // returns `None` when the legacy name equals the canonical one,
    // `carry_forward` refuses a source equal to its destination, and `copy_into`
    // skips a path the destination already has — and it was measured: no single
    // mutation in any of the three changes the observable result, so such a test
    // can never go red and would assert nothing. The layer that does decide it
    // owns the test (`legacy_appdata::tests::matching_legacy_root_is_none_
    // before_the_rename_lands`), and the case that *is* mutation-sensitive here
    // — reading the wrong pre-rename root — is covered by the channel-pairing
    // test above.

    // -----------------------------------------------------------------------
    // macOS — the handoff
    // -----------------------------------------------------------------------

    #[test]
    fn the_capture_leg_runs_under_the_old_identifier_and_the_replay_leg_under_the_new_one() {
        assert_eq!(leg_for(brand::LEGACY.bundle_id), HandoffLeg::Capture);
        assert_eq!(leg_for(brand::LEGACY.bundle_id_dev), HandoffLeg::Capture);
        assert_eq!(leg_for(post_rename().bundle_id), HandoffLeg::Replay);
        assert_eq!(leg_for(post_rename().bundle_id_dev), HandoffLeg::Replay);
    }

    #[test]
    fn only_app_owned_keys_are_eligible_in_either_spelling() {
        let canonical = post_rename();
        for owned in [
            format!("{}editor::layout", brand::LEGACY.storage_prefix),
            format!("{}theme", brand::LEGACY.storage_key_prefix),
            format!("{}editor::layout", canonical.storage_prefix),
            format!("{}theme", canonical.storage_key_prefix),
        ] {
            assert!(is_app_owned_key(&owned, &canonical), "{owned} is ours");
        }
        for foreign in [
            "github.com:session",
            "ally-supabase-auth-token",
            "store:editor::layout",
        ] {
            assert!(
                !is_app_owned_key(foreign, &canonical),
                "{foreign} belongs to a third-party page and is not ours to move"
            );
        }
    }

    #[test]
    fn handoff_round_trips_the_app_owned_keys_and_leaves_foreign_ones_behind() {
        let temp = tempfile::tempdir().unwrap();
        let old_app_data = temp.path().join(brand::LEGACY.bundle_id);
        let canonical = post_rename();
        let owned_store = format!("{}editor::layout", brand::LEGACY.storage_prefix);
        let owned_bare = format!("{}theme", brand::LEGACY.storage_key_prefix);

        let written = capture(
            &old_app_data,
            brand::LEGACY.bundle_id,
            [
                (owned_store.clone(), "{\"split\":2}".to_string()),
                (owned_bare.clone(), "dark".to_string()),
                ("github.com:session".to_string(), "not-ours".to_string()),
            ],
            &canonical,
        )
        .unwrap();
        assert_eq!(written.entries.len(), 2);

        // What the rename actually does to the file: legacy_appdata carries it
        // into the post-rename root, where the replay leg finds it.
        let new_app_data = temp.path().join(canonical.bundle_id);
        legacy_appdata::carry_forward(&old_app_data, &new_app_data).unwrap();

        let replayed =
            pending(&new_app_data).expect("a handoff is waiting under the new identifier");
        assert_eq!(replayed.written_under, brand::LEGACY.bundle_id);
        assert_eq!(replayed.entries.get(&owned_store).unwrap(), "{\"split\":2}");
        assert_eq!(replayed.entries.get(&owned_bare).unwrap(), "dark");
        assert!(
            !replayed.entries.contains_key("github.com:session"),
            "third-party site data must never enter the handoff"
        );
    }

    #[test]
    fn a_consumed_handoff_is_not_replayed_a_second_time() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path();
        let canonical = post_rename();
        let key = format!("{}theme", brand::LEGACY.storage_key_prefix);
        capture(
            app_data,
            brand::LEGACY.bundle_id,
            [(key.clone(), "dark".to_string())],
            &canonical,
        )
        .unwrap();

        // First boot: replay, then confirm.
        assert!(pending(app_data).is_some());
        mark_consumed(app_data, canonical.bundle_id).unwrap();

        // The user then changes the value under the new identifier. A second
        // boot must not put "dark" back over it.
        assert!(
            pending(app_data).is_none(),
            "a consumed handoff must never be replayed again, or the second boot clobbers \
             everything the user changed after the first"
        );

        // Still idempotent, and the record of what happened survives.
        mark_consumed(app_data, canonical.bundle_id).unwrap();
        let on_disk: StorageHandoff =
            serde_json::from_slice(&fs::read(handoff_path(app_data)).unwrap()).unwrap();
        assert_eq!(on_disk.consumed_under.as_deref(), Some(canonical.bundle_id));
        assert_eq!(on_disk.entries.get(&key).unwrap(), "dark");
    }

    #[test]
    fn a_handoff_that_is_absent_unreadable_or_from_a_future_schema_is_simply_nothing_to_replay() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path();
        assert!(pending(app_data).is_none(), "absent");

        fs::write(handoff_path(app_data), b"{ this is not json").unwrap();
        assert!(pending(app_data).is_none(), "unparseable");

        fs::write(
            handoff_path(app_data),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": HANDOFF_SCHEMA_VERSION + 1,
                "writtenUnder": brand::LEGACY.bundle_id,
                "entries": { "termul:theme": "dark" }
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(
            pending(app_data).is_none(),
            "a schema this build cannot read"
        );
    }

    #[test]
    fn capture_replaces_an_earlier_handoff_rather_than_accumulating() {
        let temp = tempfile::tempdir().unwrap();
        let app_data = temp.path();
        let canonical = post_rename();
        let key = format!("{}theme", brand::LEGACY.storage_key_prefix);

        capture(
            app_data,
            brand::LEGACY.bundle_id,
            [(key.clone(), "light".to_string())],
            &canonical,
        )
        .unwrap();
        capture(
            app_data,
            brand::LEGACY.bundle_id,
            [(key.clone(), "dark".to_string())],
            &canonical,
        )
        .unwrap();

        let replayed = pending(app_data).unwrap();
        assert_eq!(
            replayed.entries.get(&key).unwrap(),
            "dark",
            "each boot under the old identifier sees fresher values than the last"
        );
        assert!(
            !handoff_path(app_data).with_extension("json.tmp").exists(),
            "the temp file must be renamed away, not left beside the handoff"
        );
    }
}
