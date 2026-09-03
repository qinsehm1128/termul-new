//! Single source of truth for every brand-bearing identifier that crosses a
//! persistence or wire boundary — Rust side. Mirrors `src/shared/brand.ts`.
//!
//! Two constant groups, and the distinction between them is load-bearing:
//!
//! - [`LEGACY`] — the values already on users' disks / in their agent memories.
//!   Permanent. Only ever *read*; nothing may write them again (FORBID-04).
//!   This file and `src/shared/brand.ts` are the only two non-fixture files
//!   permitted to contain a legacy brand string.
//! - [`canonical`] — the values the app writes *today*. Renaming a contract is
//!   therefore a one-line edit here, which is what makes "no repo-wide sed" a
//!   structural property rather than a slogan.
//!
//! [`canonical`] reads through an override seam so a harness test can inject
//! the post-rename value while production still emits the pre-rename one. That
//! is how a Wave-1 red can be a *real* red rather than a self-certifying
//! assertion (`assert_eq!(CONST, "same literal")` can never fail).
//!
//! The seam is `pub` rather than `#[cfg(test)]` because the harness lives in
//! `src-tauri/tests/`, which links this crate as an external dependency and so
//! never sees `cfg(test)` items. Production code must never call it; the
//! residual-scan gate greps for stray call sites.

use std::cell::Cell;


/// Every canonical brand identifier the app writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrandCanonical {
    /// `ConversationRecordV2.createdBy` discriminant.
    pub created_by: &'static str,
    /// Markdown fence language carrying an ACP plan payload.
    pub plan_fence: &'static str,
    /// Per-user-repository workspace directory name.
    pub workspace_dir: &'static str,
    /// Short display name; also the `~/Documents/<name>` path component.
    pub display_name: &'static str,
    /// Full display name / product name.
    pub display_name_full: &'static str,
    /// Desktop binary and cargo package name.
    pub package_name: &'static str,
    /// Standalone headless server binary name.
    pub server_binary: &'static str,
    /// Production bundle identifier.
    pub bundle_id: &'static str,
    /// Development bundle identifier.
    pub bundle_id_dev: &'static str,
    /// Base file name (no extension) of the desktop log file.
    pub log_file_name: &'static str,
    /// `log`/`tracing` target prefix and `RUST_LOG` directive stem.
    pub log_target: &'static str,
    /// Keychain service holding desktop general credentials.
    pub keychain_service: &'static str,
    /// Keychain service holding SSH passwords and key passphrases.
    pub keychain_ssh_service: &'static str,
    /// App-managed `~/.ssh/` host-key store, kept separate from the user's own
    /// `known_hosts` so libssh2's writer cannot drop `@cert-authority` /
    /// `@revoked` markers it does not understand.
    ///
    /// DISCOVERED DURING EXECUTE — absent from all 14 roots in
    /// `migration-plan.json` and from the analyze inventory. Renaming this file
    /// without migrating it does not fail loudly: the host-key store simply
    /// looks empty, every known host becomes "unknown", and `accept-new`
    /// silently re-trusts whatever answers — which is precisely the state a
    /// MITM needs. It must be migrated, not just renamed.
    pub ssh_known_hosts_file: &'static str,
    /// Keychain service holding the iOS pairing secret.
    pub keychain_pairing_service: &'static str,
    /// MCP server name exposed to agents.
    pub mcp_server_name: &'static str,
    /// Name of the managed scheduled-tasks agent skill.
    pub skill_name: &'static str,
    /// HTML marker identifying a skill file this app wrote.
    pub skill_marker: &'static str,
    /// On-disk key of the managed-skill manifest's ownership flag, in
    /// `<workspace_dir>/managed-skills.json`.
    ///
    /// `ManagedSkillManifestV1` carries `#[serde(rename_all = "camelCase",
    /// deny_unknown_fields)]`, so the Rust field identifier *is* this JSON key —
    /// there is no literal anywhere to grep for, and renaming the identifier is
    /// a one-token edit that compiles and silently rewrites an external
    /// contract. `deny_unknown_fields` is what makes that data loss rather than
    /// drift: a manifest already on a user's disk stops deserializing outright
    /// the instant the key moves.
    ///
    /// T-A21 moved it and bumped the manifest's `schema_version` 1 -> 2. The
    /// pre-rename key survives as a permanent `#[serde(alias)]` — the one place
    /// outside this file and `src/shared/brand.ts` allowed to spell a legacy
    /// brand value, because serde attributes take literals only and cannot read
    /// this constant. The agreement between that attribute and this constant is
    /// guarded by `tests/legacy_brand_skill_manifest.rs` rather than by a
    /// whitelist entry nothing enforces.
    pub skill_manifest_key: &'static str,
    /// frp `[[proxies]]` registration name.
    pub frp_proxy_name: &'static str,
    /// Standalone-server state root directory name (unix, lowercase).
    pub state_dir: &'static str,
    /// Standalone-server state root directory name (Windows, capitalized).
    pub state_dir_windows: &'static str,
    /// Negotiated binary WebSocket subprotocol.
    pub ws_subprotocol: &'static str,
    /// Prefix of every environment variable the app reads.
    pub env_prefix: &'static str,
    /// Prefix of every global/element id injected into third-party pages.
    pub dom_global_prefix: &'static str,
    /// Deep-link URL scheme (without `://`).
    pub deep_link_scheme: &'static str,
    /// `tauri-plugin-store` namespace prefix — `<prefix><namespace>::<key>` in
    /// the WebView's `localStorage`. Mirrors `storagePrefix` in
    /// `src/shared/brand.ts`.
    pub storage_prefix: &'static str,
    /// Prefix of every bare app-owned `localStorage` key the renderer writes
    /// outside the store plugin. Mirrors `storageKeyPrefix` in
    /// `src/shared/brand.ts`.
    ///
    /// Both prefixes exist on the Rust side for one reason: on macOS the
    /// WebView data store is partitioned by bundle identifier and cannot be
    /// moved, so the app has to read its own keys out under the old identifier
    /// and replay them under the new one — see
    /// `src/webview_storage_handoff.rs`.
    pub storage_key_prefix: &'static str,
}

/// Values already written to user disks. Permanent, read-only, never re-emitted.
///
/// Migration and compatibility-read paths are the *only* legitimate consumers.
pub const LEGACY: BrandCanonical = BrandCanonical {
    created_by: "termul",
    plan_fence: "termul-plan",
    workspace_dir: ".termul",
    display_name: "Termul",
    display_name_full: "Termul Manager",
    package_name: "termul-manager",
    server_binary: "termul-server",
    bundle_id: "com.termul-manager.app",
    bundle_id_dev: "com.termul-manager.app.dev",
    log_file_name: "termul",
    log_target: "termul",
    keychain_service: "com.termul.manager",
    keychain_ssh_service: "termul-ssh",
    ssh_known_hosts_file: "known_hosts_termul",
    keychain_pairing_service: "com.termul.remote.pairing",
    mcp_server_name: "termul",
    skill_name: "termul-scheduled-tasks",
    skill_marker: "<!-- managed-by-termul:termul-scheduled-tasks -->",
    skill_manifest_key: "managedByTermul",
    frp_proxy_name: "termul",
    state_dir: "termul",
    state_dir_windows: "Termul",
    ws_subprotocol: "termul-terminal-v2.binary",
    env_prefix: "TERMUL_",
    dom_global_prefix: "__termul",
    deep_link_scheme: "termul",
    storage_prefix: "termul-store:",
    storage_key_prefix: "termul:",
};

/// Values the app writes today.
///
/// Wave 5 flips these one contract at a time. Until a contract's flip task
/// lands, its entry here still equals the corresponding [`LEGACY`] value — that
/// gap is precisely what makes the Wave-1 harness tests go red.
pub const DEFAULT_CANONICAL: BrandCanonical = BrandCanonical {
    created_by: "se-manager",
    plan_fence: "termul-plan",
    workspace_dir: ".termul",
    display_name: "Se",
    display_name_full: "Termul Manager",
    package_name: "termul-manager",
    server_binary: "termul-server",
    bundle_id: "com.termul-manager.app",
    bundle_id_dev: "com.termul-manager.app.dev",
    log_file_name: "termul",
    log_target: "termul",
    keychain_service: "com.se-manager.app",
    keychain_ssh_service: "com.se-manager.ssh",
    ssh_known_hosts_file: "known_hosts_se-manager",
    keychain_pairing_service: "com.termul.remote.pairing",
    mcp_server_name: "se-manager",
    skill_name: "se-manager-scheduled-tasks",
    skill_marker: "<!-- managed-by-se-manager:se-manager-scheduled-tasks -->",
    skill_manifest_key: "managedBySeManager",
    frp_proxy_name: "se-manager",
    state_dir: "termul",
    state_dir_windows: "Termul",
    ws_subprotocol: "se-terminal-v2.binary",
    env_prefix: "TERMUL_",
    dom_global_prefix: "__se",
    deep_link_scheme: "termul",
    storage_prefix: "se-store:",
    storage_key_prefix: "se:",
};

thread_local! {
    /// Per-thread override. Cargo runs tests in parallel threads inside one
    /// process, so a *process*-global seam would leak one test's injected value
    /// into every sibling test. Thread-local is therefore the correct default.
    static THREAD_OVERRIDE: Cell<Option<BrandCanonical>> = const { Cell::new(None) };
}

/// The canonical brand values in force right now.
///
/// Always call this rather than caching the result in a `static` — a cached
/// value freezes before a test can override it.
pub fn canonical() -> BrandCanonical {
    THREAD_OVERRIDE
        .with(Cell::get)
        .unwrap_or(DEFAULT_CANONICAL)
}

/// Test seam: force canonical values on **this thread** until the guard drops.
///
/// Production never calls this. Harness tests use it to inject the *post*-rename
/// value while production still emits the pre-rename one, so the resulting red
/// reflects a real missing capability instead of a stale literal.
#[doc(hidden)]
#[must_use = "the override is reverted when the guard is dropped"]
pub fn override_canonical(next: BrandCanonical) -> BrandOverrideGuard {
    let previous = THREAD_OVERRIDE.with(|slot| slot.replace(Some(next)));
    BrandOverrideGuard { previous }
}

/// Reverts an [`override_canonical`] call when dropped.
#[doc(hidden)]
pub struct BrandOverrideGuard {
    previous: Option<BrandCanonical>,
}

impl Drop for BrandOverrideGuard {
    fn drop(&mut self) {
        THREAD_OVERRIDE.with(|slot| slot.set(self.previous));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_defaults_to_shipped_values() {
        assert_eq!(canonical(), DEFAULT_CANONICAL);
    }

    #[test]
    fn override_replaces_canonical_and_reverts_on_drop() {
        let before = canonical();
        {
            let _guard = override_canonical(BrandCanonical {
                created_by: "se-manager",
                ..DEFAULT_CANONICAL
            });
            assert_eq!(canonical().created_by, "se-manager");
            // Untouched fields still come from the shipped values.
            assert_eq!(canonical().plan_fence, DEFAULT_CANONICAL.plan_fence);
        }
        assert_eq!(canonical(), before);
    }

    #[test]
    fn legacy_values_are_independent_of_the_override_seam() {
        let _guard = override_canonical(BrandCanonical {
            created_by: "se-manager",
            workspace_dir: ".se-manager",
            ..DEFAULT_CANONICAL
        });
        assert_eq!(LEGACY.created_by, "termul");
        assert_eq!(LEGACY.workspace_dir, ".termul");
    }

    /// The whole harness runs under `cargo test`'s parallel thread pool. If one
    /// test's injected value were visible to a sibling, every red in Wave 1
    /// would be non-deterministic and the ledger would prove nothing.
    #[test]
    fn override_does_not_leak_into_other_threads() {
        let _guard = override_canonical(BrandCanonical {
            created_by: "se-manager",
            ..DEFAULT_CANONICAL
        });
        assert_eq!(canonical().created_by, "se-manager");

        let observed = std::thread::spawn(|| canonical().created_by).join().unwrap();
        assert_eq!(observed, DEFAULT_CANONICAL.created_by);
    }
}
