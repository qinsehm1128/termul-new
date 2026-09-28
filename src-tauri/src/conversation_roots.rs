//! The hosts' Conversation roots, including what a pre-rename install left
//! behind. These read the brand seam and the legacy app-data module, which
//! is why they live on the host side rather than with the Conversation domain.

use std::path::{Path, PathBuf};

use crate::conversation::HostConversationRoots;

/// The desktop host's roots, including everything a pre-rename install left
/// behind (M-01, M-02, M-06).
///
/// `state_root` is Tauri's `app_data_dir()`, which is named from the bundle
/// identifier: renaming the identifier moves it, and every byte under the
/// old one becomes unreachable. So this constructor does two distinct
/// things before returning:
///
/// 1. **Carries the matching pre-rename tree forward** (copy-only, never
///    overwriting, never deleting — see [`crate::legacy_appdata`]). prod
///    carries prod and dev carries dev; the two are separate installs.
/// 2. **Declares both pre-rename identifier trees, and the pre-rename
///    `~/Documents/<display name>` workspace root, as legacy-readable**, so
///    the migration inventory and the startup detector can see data under
///    either one. Declaring is read-only: the user's `~/Documents` tree is
///    never moved or copied by this call.
///
/// Resolving the brand seam here is deliberate and load-bearing:
/// `brand::canonical()` is thread-local, so it must be read on the thread
/// that owns the override, never inside a spawned closure (FORBID-07). This
/// runs on the caller's thread — the Tauri `setup` thread in production.
#[must_use]
pub fn desktop(state_root: PathBuf, workspace_base: PathBuf) -> HostConversationRoots {
    let (legacy_appdata_roots, legacy_workspace_bases) = if crate::brand::is_canary_build() {
        // Canary is a parallel install, not a rename/migration target. It
        // must never inspect or copy production conversation roots.
        (Vec::new(), Vec::new())
    } else {
        let legacy_appdata_roots = crate::legacy_appdata::legacy_appdata_roots(&state_root);
        if let Some(source) = crate::legacy_appdata::matching_legacy_root(&state_root) {
            match crate::legacy_appdata::carry_forward(&source, &state_root) {
                Ok(report) if report.is_noop() => {}
                Ok(report) => log::info!(
                    "[legacy-appdata] carried the pre-rename app data root forward from {} copied={} already_present={} skipped_links={}",
                    source.display(),
                    report.copied,
                    report.already_present,
                    report.skipped_links
                ),
                // Non-fatal by design: the legacy tree is still on disk and is
                // still declared below, so a failed copy costs "the merge has
                // more to do", never data. Refusing to launch would not make
                // the user's data any more reachable.
                Err(error) => log::error!(
                    "[legacy-appdata] could not carry {} forward into {}: {error}",
                    source.display(),
                    state_root.display()
                ),
            }
        }
        (
            legacy_appdata_roots,
            legacy_workspace_base(&workspace_base).into_iter().collect(),
        )
    };
    let carried_from_state_roots = crate::legacy_appdata::matching_legacy_root(&state_root)
        .into_iter()
        .collect();
    HostConversationRoots {
        state_root,
        workspace_base,
        // Unchanged: the desktop host has never had standalone-shaped
        // legacy leaves, and the carried-forward tree is reached through
        // `host_state_root` like it always was. See
        // `legacy_appdata_roots`'s doc for why the pre-rename roots do not
        // belong here.
        legacy_session_roots: Vec::new(),
        legacy_workspace_manifest_roots: Vec::new(),
        legacy_appdata_roots,
        legacy_workspace_bases,
        carried_from_state_roots,
    }
}

#[must_use]
pub fn standalone(
    state_root: PathBuf,
    workspace_base: PathBuf,
    legacy_session_root: Option<PathBuf>,
    legacy_workspace_manifest_root: Option<PathBuf>,
) -> HostConversationRoots {
    // `<project_root>/<display name>` is the standalone twin of
    // `~/Documents/<display name>` and is named by the same identity, so
    // the same pre-rename sibling can be sitting next to it (T-A16). Read
    // on the caller's thread like the desktop constructor (FORBID-07), and
    // read-only in exactly the same sense: the user's workspaces are never
    // moved on the strength of this field.
    let legacy_workspace_bases = legacy_workspace_base(&workspace_base).into_iter().collect();
    let carried_from_state_roots = crate::legacy_appdata::matching_legacy_root(&state_root)
        .into_iter()
        .collect();
    HostConversationRoots {
        state_root,
        workspace_base,
        legacy_session_roots: legacy_session_root.into_iter().collect(),
        legacy_workspace_manifest_roots: legacy_workspace_manifest_root.into_iter().collect(),
        // The standalone host names its state root from `state_dir`, not
        // from a bundle identifier. Its own legacy fallback is T-M07's,
        // applied by the caller before it gets here.
        legacy_appdata_roots: Vec::new(),
        legacy_workspace_bases,
        carried_from_state_roots,
    }
}

/// The pre-rename sibling of `workspace_base` — `~/Documents/<old display
/// name>` — when it exists on disk (M-06).
///
/// `workspace_base` is `<documents>/<display_name>`, so the legacy root is its
/// sibling under the same parent. `None` when the final component is not the
/// canonical display name (the user pointed `SE_CONVERSATION_WORKSPACE_ROOT`
/// somewhere of their own), when the rename has not landed yet and the two
/// names are equal, or when nothing is there.
///
/// This is a *detection* helper and nothing more. The directory it returns is
/// full of the user's own project files; the merge never moves or copies it,
/// and the caller stores the result in a field documented read-only.
///
/// Reads the brand seam, so it must be called on the thread that owns it
/// (FORBID-07).
///
/// Public so `migration_detect` reports the same root this constructor
/// declares. A second copy of the "is the final component the canonical display
/// name?" rule would let the banner list a directory the host never registered.
#[must_use]
pub fn legacy_workspace_base(workspace_base: &Path) -> Option<PathBuf> {
    let canonical = crate::brand::canonical();
    let legacy_name = crate::brand::LEGACY.display_name;
    if legacy_name == canonical.display_name {
        return None;
    }
    if workspace_base.file_name()?.to_str()? != canonical.display_name {
        return None;
    }
    let legacy_base = workspace_base.parent()?.join(legacy_name);
    legacy_base.is_dir().then_some(legacy_base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::{self, BrandCanonical};
    use crate::conversation::migration::{
        LegacyRootConfiguration, MigrationHostMode, MigrationPhase,
    };
    use crate::conversation::ConversationBootstrap;
    use std::fs;

    fn post_rename() -> BrandCanonical {
        BrandCanonical {
            bundle_id: "com.se-manager.app",
            bundle_id_dev: "com.se-manager.app.dev",
            display_name: "Se",
            ..brand::DEFAULT_CANONICAL
        }
    }

    /// M-06. `~/Documents/<old display name>` is named by `display_name`, an
    /// identity completely separate from the bundle identifier that names
    /// `app_data_dir`.
    #[test]
    fn legacy_workspace_base_finds_the_pre_rename_documents_root() {
        let temp = tempfile::tempdir().unwrap();
        let documents = temp.path();
        fs::create_dir_all(documents.join(brand::LEGACY.display_name)).unwrap();
        let _brand = brand::override_canonical(post_rename());

        assert_eq!(
            legacy_workspace_base(&documents.join(post_rename().display_name)),
            Some(documents.join(brand::LEGACY.display_name))
        );
    }

    #[test]
    fn legacy_workspace_base_ignores_a_user_supplied_root() {
        let temp = tempfile::tempdir().unwrap();
        let documents = temp.path();
        fs::create_dir_all(documents.join(brand::LEGACY.display_name)).unwrap();
        let _brand = brand::override_canonical(post_rename());

        // SE_CONVERSATION_WORKSPACE_ROOT pointed somewhere of the user's
        // own choosing: there is no rename relationship to infer.
        assert_eq!(
            legacy_workspace_base(&documents.join("my-own-projects")),
            None
        );
    }

    #[test]
    fn legacy_workspace_base_is_none_before_the_rename_lands() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(brand::DEFAULT_CANONICAL.display_name)).unwrap();
        assert_eq!(
            legacy_workspace_base(&temp.path().join(brand::DEFAULT_CANONICAL.display_name)),
            None,
            "with canonical == legacy the sibling IS the canonical root"
        );
    }

    /// The pre-rename `app_data_dir` trees are reported for detection, but they
    /// must never reach `LegacyRootConfiguration`: `standalone_session_roots`
    /// entries are scanned as `acp-sessions` leaves, and the vector
    /// deliberately includes the install channel this process is not.
    #[test]
    fn pre_rename_appdata_roots_are_declared_for_detection_but_never_migrated() {
        let temp = tempfile::tempdir().unwrap();
        let support = temp.path().join("Application Support");
        for name in [brand::LEGACY.bundle_id, brand::LEGACY.bundle_id_dev] {
            fs::create_dir_all(support.join(name)).unwrap();
        }
        let state_root = support.join(post_rename().bundle_id);
        fs::create_dir_all(&state_root).unwrap();
        let _brand = brand::override_canonical(post_rename());

        let roots = desktop(state_root.clone(), temp.path().join("Se"));

        assert_eq!(
            roots.legacy_appdata_roots,
            vec![
                support.join(brand::LEGACY.bundle_id),
                support.join(brand::LEGACY.bundle_id_dev),
            ],
            "both identifier trees must be visible to the detector"
        );
        assert!(
            roots.legacy_session_roots.is_empty(),
            "an app_data_dir root is not an acp-sessions leaf and the dev tree \
             must not be merged into a release install; got {:?}",
            roots.legacy_session_roots
        );

        let configuration = LegacyRootConfiguration {
            host_state_root: roots.state_root.clone(),
            standalone_session_roots: roots.legacy_session_roots.clone(),
            standalone_workspace_manifest_roots: roots.legacy_workspace_manifest_roots.clone(),
        };
        for spec in configuration.known_roots() {
            assert!(
                spec.path.starts_with(&state_root),
                "the inventory must only ever scan under the canonical root, got {}",
                spec.path.display()
            );
        }
    }

    /// Bootstrap adopts a carried-forward journal only from the root named
    /// here, so it must be the channel-matched pre-rename root.
    #[test]
    fn desktop_names_the_matching_pre_rename_root_as_carried_from() {
        let temp = tempfile::tempdir().unwrap();
        let support = temp.path().join("Application Support");
        for name in [brand::LEGACY.bundle_id, brand::LEGACY.bundle_id_dev] {
            fs::create_dir_all(support.join(name)).unwrap();
        }
        let _brand = brand::override_canonical(post_rename());

        let roots = desktop(
            support.join(post_rename().bundle_id_dev),
            temp.path().join("Se"),
        );

        assert_eq!(
            roots.carried_from_state_roots,
            vec![support.join(brand::LEGACY.bundle_id_dev)],
            "dev carries dev; the release tree is declared but never adopted"
        );
    }

    /// The shipped v0.6.0 crash, end to end.
    ///
    /// A user whose data lived under the pre-rename bundle identifier gets the
    /// whole tree carried forward on first launch — journal included. That
    /// journal's key was derived from the OLD root, so startup used to abort
    /// with `MIGRATION_IDEMPOTENCY_CONFLICT` on data that was entirely intact.
    /// Uses the real directory names so a future rename cannot quietly make
    /// this test stop covering the case it was written for.
    #[test]
    fn a_journal_carried_forward_from_the_pre_rename_root_does_not_abort_startup() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path();
        let canonical_root = parent.join(brand::canonical().bundle_id_dev);
        let legacy_root = parent.join(brand::LEGACY.bundle_id_dev);

        // Migrate once under the legacy identifier, exactly as the pre-rename
        // build did.
        let first = ConversationBootstrap::run(
            desktop(legacy_root.clone(), parent.join("visible")),
            MigrationHostMode::Desktop,
        )
        .expect("the pre-rename install migrates cleanly");
        assert_eq!(first.migration_phase, MigrationPhase::ObservationWindow);

        // Rename lands: the canonical root does not exist yet, so bootstrap
        // carries the legacy tree — journal and all — forward into it.
        let carried = ConversationBootstrap::run(
            desktop(canonical_root.clone(), parent.join("visible")),
            MigrationHostMode::Desktop,
        )
        .expect("a carried-forward journal must not abort startup");
        assert_eq!(carried.migration_phase, MigrationPhase::ObservationWindow);

        // And the legacy tree is still there, untouched.
        assert!(legacy_root.join("conversation-migrations").is_dir());
    }
}
