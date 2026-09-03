//! Carry a pre-rename `app_data_dir` tree forward to the post-rename one
//! (M-01, M-02).
//!
//! # What moves and what does not
//!
//! Tauri resolves `app_data_dir()` from `tauri.conf.json`'s `identifier`.
//! Renaming the bundle identifier therefore moves the whole root: on macOS from
//! `~/Library/Application Support/<old id>` to `.../<new id>`, on Linux from
//! `$XDG_DATA_HOME/<old id>`, on Windows from `%APPDATA%\<old id>`. Nothing in
//! the app computes the old path any more, so every byte under it — the
//! conversation store, the remote-tunnel token, the scheduled tasks, the ACP
//! catalog and its downloaded binaries and npm packages, the remote intent
//! store — is stranded without a single error being raised.
//!
//! The existing conversation legacy-stage pipeline cannot carry this. Its three
//! source kinds (`inventory.rs`) are `acp-sessions`, `acp-chat-history` and
//! `workspace-manifests`, and each scanner filters to conversation data; it has
//! no entry point for the other subdirectories at all. So this module is a
//! plain, whole-tree, copy-only carry-forward that runs *beside* that pipeline
//! rather than inside it.
//!
//! # prod and dev are two installs, not one
//!
//! `bundle_id` and `bundle_id_dev` name two separate roots that a user may hold
//! simultaneously, with genuinely different contents (a dev build's
//! `remote-tunnel/config.json` is not the release build's). A running process
//! resolves exactly one of them, so the carry-forward is identifier-*matched*:
//! prod carries prod, dev carries dev. Merging them into one root would let a
//! dev experiment overwrite — or be overwritten by — real user data.
//!
//! Both trees are still *declared* as legacy-readable roots
//! (`HostConversationRoots::desktop`), because detection and the merge
//! inventory have to be able to tell the user about either one.
//!
//! # Copy only. Never delete, never overwrite (FORBID-05)
//!
//! A destination file that already exists is left alone and the source is left
//! alone: the post-rename install may have written newer data at that path, and
//! the pre-rename tree stays on disk so a user who reverts to the old build
//! still finds everything. That also makes the pass idempotent — running it on
//! every startup converges instead of thrashing.
//!
//! Symlinks are skipped rather than followed, so a link planted inside the
//! legacy tree cannot make the copy write outside the destination root.
//!
//! `std::fs::copy` carries the source's permission bits on unix, which is what
//! keeps `remote-tunnel/secrets.json` — the frp auth token and the remote
//! access token — at 0600 instead of widening it to every local account.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::brand;

/// What one carry-forward pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CarryForwardReport {
    /// Files copied into the canonical root on this pass.
    pub copied: usize,
    /// Files skipped because the canonical root already had that path.
    pub already_present: usize,
    /// Entries skipped because they were symlinks or reparse points.
    pub skipped_links: usize,
}

impl CarryForwardReport {
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.copied == 0
    }
}

/// The pre-rename sibling of `canonical_root` that corresponds to the *same*
/// install channel, if it exists on disk.
///
/// `None` when the canonical root's final component is neither of the canonical
/// bundle identifiers (an injected or relocated root), when the legacy name
/// equals the canonical one (no rename has happened yet), or when nothing is
/// there.
///
/// Reads the brand seam, so it must be called on the thread that owns it
/// (FORBID-07).
#[must_use]
pub fn matching_legacy_root(canonical_root: &Path) -> Option<PathBuf> {
    let canonical = brand::canonical();
    let parent = canonical_root.parent()?;
    let name = canonical_root.file_name()?.to_str()?;

    let legacy_name = if name == canonical.bundle_id {
        brand::LEGACY.bundle_id
    } else if name == canonical.bundle_id_dev {
        brand::LEGACY.bundle_id_dev
    } else {
        return None;
    };
    if legacy_name == name {
        return None;
    }

    let legacy_root = parent.join(legacy_name);
    legacy_root.is_dir().then_some(legacy_root)
}

/// Both pre-rename identifier trees beside `canonical_root`, prod first, in the
/// order a detector should report them. Only roots that exist are returned.
///
/// This is the *read* view — it deliberately includes the channel the running
/// process is not, because a user who renamed while holding both installs has
/// to be told about both. Copying is [`matching_legacy_root`]'s narrower job.
///
/// Reads the brand seam, so it must be called on the thread that owns it
/// (FORBID-07).
#[must_use]
pub fn legacy_appdata_roots(canonical_root: &Path) -> Vec<PathBuf> {
    let Some(parent) = canonical_root.parent() else {
        return Vec::new();
    };
    let canonical = brand::canonical();
    [
        (brand::LEGACY.bundle_id, canonical.bundle_id),
        (brand::LEGACY.bundle_id_dev, canonical.bundle_id_dev),
    ]
    .into_iter()
    .filter(|(legacy, current)| legacy != current)
    .map(|(legacy, _)| parent.join(legacy))
    .filter(|root| root.is_dir())
    .collect()
}

/// Copy every file under `legacy_root` that the canonical root does not already
/// have. Never deletes, never overwrites, never follows a link out of the tree.
///
/// Takes no brand values of its own — the caller resolves those — so it is safe
/// to call from anywhere.
pub fn carry_forward(legacy_root: &Path, canonical_root: &Path) -> io::Result<CarryForwardReport> {
    let mut report = CarryForwardReport::default();
    if legacy_root == canonical_root || !legacy_root.is_dir() {
        return Ok(report);
    }
    copy_into(legacy_root, canonical_root, &mut report)?;
    Ok(report)
}

fn copy_into(source: &Path, destination: &Path, report: &mut CarryForwardReport) -> io::Result<()> {
    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        // A directory that vanished mid-walk is not a reason to strand the rest
        // of the tree.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let from = entry.path();
        let metadata = fs::symlink_metadata(&from)?;
        if metadata.file_type().is_symlink() {
            report.skipped_links += 1;
            continue;
        }
        let to = destination.join(entry.file_name());
        if metadata.is_dir() {
            // `symlink_metadata` on the destination: a symlinked directory in
            // the destination would otherwise redirect the whole subtree.
            match fs::symlink_metadata(&to) {
                Ok(existing) if existing.file_type().is_symlink() => {
                    report.skipped_links += 1;
                    continue;
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(&to)?,
                Err(error) => return Err(error),
            }
            copy_into(&from, &to, report)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        if fs::symlink_metadata(&to).is_ok() {
            report.already_present += 1;
            continue;
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        // `fs::copy` carries the source's permission bits on unix, which is how
        // `remote-tunnel/secrets.json` stays at 0600.
        fs::copy(&from, &to)?;
        report.copied += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::BrandCanonical;

    fn post_rename() -> BrandCanonical {
        BrandCanonical {
            bundle_id: "com.se-manager.app",
            bundle_id_dev: "com.se-manager.app.dev",
            ..brand::DEFAULT_CANONICAL
        }
    }

    #[test]
    fn matching_legacy_root_pairs_prod_with_prod_and_dev_with_dev() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        for name in [
            brand::LEGACY.bundle_id,
            brand::LEGACY.bundle_id_dev,
            post_rename().bundle_id,
            post_rename().bundle_id_dev,
        ] {
            fs::create_dir_all(base.join(name)).unwrap();
        }
        let _brand = brand::override_canonical(post_rename());

        assert_eq!(
            matching_legacy_root(&base.join(post_rename().bundle_id)),
            Some(base.join(brand::LEGACY.bundle_id))
        );
        assert_eq!(
            matching_legacy_root(&base.join(post_rename().bundle_id_dev)),
            Some(base.join(brand::LEGACY.bundle_id_dev))
        );
    }

    /// With `canonical == legacy` there is nothing to carry and the source
    /// would be the destination — a pass that copies a live root onto itself.
    ///
    /// This needed no injection until T-A22 flipped `bundle_id`; the state it
    /// pins is now only reachable by injecting `LEGACY` wholesale, which
    /// reproduces the pre-flip build exactly. Asserting it against today's
    /// shipped values instead would assert a different thing entirely: the
    /// `legacy_name == name` guard would never be reached, and deleting that
    /// guard would leave the test green.
    #[test]
    fn matching_legacy_root_is_none_before_the_rename_lands() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(brand::LEGACY.bundle_id)).unwrap();
        let _brand = brand::override_canonical(brand::LEGACY);
        assert_eq!(
            matching_legacy_root(&temp.path().join(brand::LEGACY.bundle_id)),
            None
        );
    }

    #[test]
    fn carry_forward_copies_missing_files_and_never_overwrites_or_deletes() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("legacy");
        let canonical = temp.path().join("canonical");
        fs::create_dir_all(legacy.join("remote-tunnel")).unwrap();
        fs::create_dir_all(canonical.join("remote-tunnel")).unwrap();
        fs::write(legacy.join("remote-tunnel/config.json"), b"legacy-config").unwrap();
        fs::write(legacy.join("remote-tunnel/secrets.json"), b"legacy-secrets").unwrap();
        // Already written by the post-rename install: must survive untouched.
        fs::write(canonical.join("remote-tunnel/config.json"), b"newer").unwrap();

        let report = carry_forward(&legacy, &canonical).unwrap();
        assert_eq!(report.copied, 1);
        assert_eq!(report.already_present, 1);
        assert_eq!(
            fs::read(canonical.join("remote-tunnel/config.json")).unwrap(),
            b"newer"
        );
        assert_eq!(
            fs::read(canonical.join("remote-tunnel/secrets.json")).unwrap(),
            b"legacy-secrets"
        );
        // The source is still complete.
        assert_eq!(
            fs::read(legacy.join("remote-tunnel/config.json")).unwrap(),
            b"legacy-config"
        );
        assert!(legacy.join("remote-tunnel/secrets.json").is_file());

        // Idempotent.
        let second = carry_forward(&legacy, &canonical).unwrap();
        assert!(second.is_noop(), "a second pass must copy nothing: {second:?}");
    }

    #[cfg(unix)]
    #[test]
    fn carry_forward_preserves_the_secrets_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("legacy");
        let canonical = temp.path().join("canonical");
        fs::create_dir_all(legacy.join("remote-tunnel")).unwrap();
        let source = legacy.join("remote-tunnel/secrets.json");
        fs::write(&source, b"{\"frpToken\":\"t\"}").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();

        carry_forward(&legacy, &canonical).unwrap();

        let mode = fs::metadata(canonical.join("remote-tunnel/secrets.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "got {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn carry_forward_skips_symlinks_instead_of_following_them_out_of_the_tree() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("stolen.txt"), b"not yours").unwrap();
        let legacy = temp.path().join("legacy");
        fs::create_dir_all(&legacy).unwrap();
        std::os::unix::fs::symlink(&outside, legacy.join("escape")).unwrap();
        fs::write(legacy.join("real.json"), b"{}").unwrap();
        let canonical = temp.path().join("canonical");

        let report = carry_forward(&legacy, &canonical).unwrap();
        assert_eq!(report.copied, 1);
        assert_eq!(report.skipped_links, 1);
        assert!(canonical.join("real.json").is_file());
        assert!(!canonical.join("escape").exists());
    }

    #[test]
    fn legacy_appdata_roots_reports_both_identifier_trees_that_exist() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        fs::create_dir_all(base.join(brand::LEGACY.bundle_id)).unwrap();
        fs::create_dir_all(base.join(brand::LEGACY.bundle_id_dev)).unwrap();
        let canonical = base.join(post_rename().bundle_id);
        fs::create_dir_all(&canonical).unwrap();
        let _brand = brand::override_canonical(post_rename());

        assert_eq!(
            legacy_appdata_roots(&canonical),
            vec![
                base.join(brand::LEGACY.bundle_id),
                base.join(brand::LEGACY.bundle_id_dev),
            ]
        );
    }

    /// The read-side twin of the guard above, and injected for the same reason
    /// since T-A22. Both trees are planted so that dropping the `legacy !=
    /// current` filter reports the user's live roots as legacy data to merge.
    #[test]
    fn legacy_appdata_roots_is_empty_before_the_rename_lands() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(brand::LEGACY.bundle_id)).unwrap();
        fs::create_dir_all(temp.path().join(brand::LEGACY.bundle_id_dev)).unwrap();
        let _brand = brand::override_canonical(brand::LEGACY);
        assert!(
            legacy_appdata_roots(&temp.path().join(brand::LEGACY.bundle_id)).is_empty(),
            "with canonical == legacy there is no separate legacy root to read"
        );
    }
}
