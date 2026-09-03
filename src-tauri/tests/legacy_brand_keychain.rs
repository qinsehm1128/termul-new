//! T-H08 — OS-keychain service names across the rename.
//!
//! # Why this reads the entries off disk instead of inlining a literal
//!
//! The app stores credentials under **two** different brand spellings:
//! `com.termul.manager` (project environment ciphertext, `src/secure_storage.rs:4`)
//! and `termul-ssh` (SSH passwords and private-key passphrases,
//! `src/ssh/credential_store.rs:8`). The second spelling shares no substring
//! shape with the first, which is exactly why it is easy to miss — and it holds
//! the highest-value secrets in the app. Losing it means every SSH profile
//! silently stops authenticating, with no error that names a cause.
//!
//! An assertion written as `assert_eq!(SERVICE_NAME, "termul-ssh")` is a copy of
//! the constant it checks and cannot fail. So the subject here is
//! `tests/fixtures/legacy-brand/keychain-entries.json`, a sha256-frozen record
//! of both services and their key shapes, and the expectations come from
//! `brand::LEGACY` / `brand::canonical()`.
//!
//! # Why these reds are structural rather than behavioural
//!
//! The brief asked for a behavioural test: pre-seed the legacy entries, inject
//! the canonical service names, and assert the new names read the values while
//! the legacy entries survive. That test cannot be written today, for two
//! independent reasons — both verified, not assumed:
//!
//! 1. **No reachable production seam.** `mod secure_storage` and `mod ssh` are
//!    private to `termul_manager_lib` (`src/lib.rs:20`, `src/lib.rs:30`) and
//!    neither is re-exported. `keyring_get`/`keyring_set` are additionally
//!    `pub(crate)`. Nothing under `tests/` can call the credential paths at all,
//!    directly or through the web routes — no HTTP handler touches them.
//!
//! 2. **No injection point.** Both modules call `keyring::Entry::new(SERVICE_NAME, key)`
//!    directly, so the backend is whatever the compile-time cargo feature
//!    selected — on macOS the user's real login keychain. `keyring::mock` (which
//!    keyring 3.6 exposes unconditionally, no feature flag needed) cannot stand
//!    in for it: its builder reports `CredentialPersistence::EntryOnly` and
//!    hands every `Entry::new` a *fresh empty* credential, so a pre-seeded
//!    legacy keychain is not representable. `set_default_credential_builder` is
//!    also process-global (`RwLock`), not thread-local, so it does not compose
//!    with the thread-local brand seam the way the other harness files rely on.
//!
//! `fake_keychain_backend_holds_the_legacy_entries_the_canonical_service_cannot_see`
//! below builds the store keyring *does* let a client supply, seeds it from the
//! frozen fixture, and executes the lookup — proving the mechanism works and
//! measuring the exact gap. The remaining tests assert the file-level shape the
//! Wave-4 change has to produce. See the report for the production seam this
//! needs.

use std::any::Any;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use keyring::credential::{Credential, CredentialApi, CredentialBuilder, CredentialBuilderApi};
use keyring::{Entry, Error};
use serde_json::Value;

use termul_manager_lib::brand::{self, BrandCanonical, DEFAULT_CANONICAL};

/// Keychain service names the app writes *after* the rename.
fn post_rename() -> BrandCanonical {
    BrandCanonical {
        keychain_service: "com.se-manager.app",
        keychain_ssh_service: "com.se-manager.ssh",
        ..DEFAULT_CANONICAL
    }
}

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/legacy-brand/keychain-entries.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()));
    serde_json::from_str(&raw).expect("keychain-entries.json parses")
}

fn service_entry(fixture: &Value, service: &str) -> Value {
    fixture["services"]
        .as_array()
        .expect("fixture carries a services array")
        .iter()
        .find(|candidate| candidate["service"] == service)
        .unwrap_or_else(|| panic!("frozen fixture has no service named {service:?}"))
        .clone()
}

fn production_source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// An injectable keychain backend — the thing production does not accept.
// ---------------------------------------------------------------------------

/// A `(service, user)`-keyed store, i.e. what a real OS keychain is and what
/// `keyring::mock` is not.
#[derive(Default)]
struct FixtureKeychain {
    secrets: Mutex<BTreeMap<(String, String), Vec<u8>>>,
}

impl FixtureKeychain {
    fn seed(&self, service: &str, user: &str, secret: &str) {
        self.secrets
            .lock()
            .unwrap()
            .insert((service.to_string(), user.to_string()), secret.into());
    }

    fn contains(&self, service: &str, user: &str) -> bool {
        self.secrets
            .lock()
            .unwrap()
            .contains_key(&(service.to_string(), user.to_string()))
    }
}

struct FixtureCredential {
    store: Arc<FixtureKeychain>,
    key: (String, String),
}

impl CredentialApi for FixtureCredential {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
        self.store
            .secrets
            .lock()
            .unwrap()
            .insert(self.key.clone(), secret.to_vec());
        Ok(())
    }

    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        self.store
            .secrets
            .lock()
            .unwrap()
            .get(&self.key)
            .cloned()
            .ok_or(Error::NoEntry)
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        self.store
            .secrets
            .lock()
            .unwrap()
            .remove(&self.key)
            .map(|_| ())
            .ok_or(Error::NoEntry)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct FixtureKeychainBuilder {
    store: Arc<FixtureKeychain>,
}

impl CredentialBuilderApi for FixtureKeychainBuilder {
    fn build(
        &self,
        _target: Option<&str>,
        service: &str,
        user: &str,
    ) -> keyring::Result<Box<Credential>> {
        Ok(Box::new(FixtureCredential {
            store: Arc::clone(&self.store),
            key: (service.to_string(), user.to_string()),
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Install the fixture-backed store once for this test binary.
///
/// `set_default_credential_builder` is process-global, so this must happen
/// exactly once and must never be undone — otherwise a sibling test in this
/// binary could reach the developer's real login keychain.
fn installed_keychain() -> &'static Arc<FixtureKeychain> {
    static STORE: OnceLock<Arc<FixtureKeychain>> = OnceLock::new();
    STORE.get_or_init(|| {
        let store = Arc::new(FixtureKeychain::default());
        let builder: Box<CredentialBuilder> = Box::new(FixtureKeychainBuilder {
            store: Arc::clone(&store),
        });
        keyring::set_default_credential_builder(builder);

        // Seed exactly what the frozen fixture records a pre-rename install left
        // behind — under the legacy service names, because that is where they
        // are.
        let fixture = fixture();
        for service in fixture["services"]
            .as_array()
            .expect("fixture carries a services array")
        {
            let name = service["service"].as_str().expect("service name");
            for entry in service["entries"].as_array().expect("service entries") {
                store.seed(
                    name,
                    entry["key"].as_str().expect("entry key"),
                    entry["value"].as_str().expect("entry value"),
                );
            }
        }
        store
    })
}

// ---------------------------------------------------------------------------
// Guards over the frozen record
// ---------------------------------------------------------------------------

/// The fixture and `brand::LEGACY` must agree on both spellings.
///
/// Neither operand is a copy of the other: the fixture is sha256-frozen by
/// `legacy_brand_fixture_manifest.rs` and the expectations are read out of the
/// brand seam at runtime. Editing either one alone turns this red.
#[test]
fn frozen_keychain_record_matches_the_recorded_legacy_brand() {
    let fixture = fixture();
    let desktop = service_entry(&fixture, brand::LEGACY.keychain_service);
    let ssh = service_entry(&fixture, brand::LEGACY.keychain_ssh_service);

    assert_ne!(
        brand::LEGACY.keychain_service,
        brand::LEGACY.keychain_ssh_service,
        "the two keychain services are different spellings; collapsing them \
         into one would strand every SSH credential"
    );

    assert!(
        desktop["entries"]
            .as_array()
            .expect("desktop entries")
            .iter()
            .all(|entry| entry["key"].as_str().is_some_and(|key| {
                key.starts_with("project:") && key.contains(":env:")
            })),
        "desktop keychain keys are project environment ciphertext: {desktop}"
    );

    let ssh_keys: Vec<&str> = ssh["entries"]
        .as_array()
        .expect("ssh entries")
        .iter()
        .map(|entry| entry["key"].as_str().expect("ssh entry key"))
        .collect();
    assert!(
        ssh_keys
            .iter()
            .all(|key| key.ends_with("-password") || key.ends_with("-passphrase")),
        "ssh keychain keys are {{profileId}}-password / -passphrase: {ssh_keys:?}"
    );
    for profile_id in fixture["ssh_profile_ids"]
        .as_array()
        .expect("fixture lists the ssh profile ids")
    {
        let profile_id = profile_id.as_str().expect("profile id");
        assert!(
            ssh_keys.iter().any(|key| key.starts_with(profile_id)),
            "profile {profile_id} has no credential in the frozen record"
        );
    }
}

/// Measures the gap, and proves the injection mechanism Wave 4 needs exists in
/// the dependency.
///
/// A `(service, user)`-keyed store handed to `keyring::set_default_credential_builder`
/// *does* back `keyring::Entry` — this is the seam production must accept. With
/// it seeded from the frozen record, the legacy service reads the SSH password
/// and the canonical service reads nothing, because no migration has copied
/// anything across. That second half is the user-visible failure the rename
/// causes, executed rather than described.
#[test]
fn fake_keychain_backend_holds_the_legacy_entries_the_canonical_service_cannot_see() {
    let store = installed_keychain();
    let fixture = fixture();
    let profile_id = fixture["ssh_profile_ids"][0]
        .as_str()
        .expect("fixture lists the ssh profile ids");
    let password_key = format!("{profile_id}-password");
    let expected = service_entry(&fixture, brand::LEGACY.keychain_ssh_service)["entries"]
        .as_array()
        .expect("ssh entries")
        .iter()
        .find(|entry| entry["key"] == Value::String(password_key.clone()))
        .expect("the frozen record carries this profile's password")["value"]
        .as_str()
        .expect("password value")
        .to_string();

    let legacy = Entry::new(brand::LEGACY.keychain_ssh_service, &password_key)
        .expect("build a legacy-service entry");
    assert_eq!(
        legacy.get_password().expect("legacy SSH password is present"),
        expected,
        "the injectable backend must round-trip the frozen legacy entries"
    );

    let canonical_service = post_rename().keychain_ssh_service;
    let canonical =
        Entry::new(canonical_service, &password_key).expect("build a canonical-service entry");
    assert!(
        matches!(canonical.get_password(), Err(Error::NoEntry)),
        "nothing has copied the SSH credentials to {canonical_service:?} — this \
         is the gap Wave 4 closes, and the reason the assertions below are red"
    );

    assert!(
        store.contains(brand::LEGACY.keychain_ssh_service, &password_key),
        "the legacy entry must still exist: a migration copies, it never deletes"
    );
}

// ---------------------------------------------------------------------------
// The ledger: file-level shape the Wave-4 change must produce
// ---------------------------------------------------------------------------

/// `brand.rs` states the rule these assert: it and `src/shared/brand.ts` are the
/// only non-fixture files permitted to contain a legacy brand string. A service
/// name frozen into a `const` cannot be renamed without stranding the entries
/// already written under it, so it has to move behind `brand::canonical()`.
fn assert_no_frozen_service_literal(relative: &str, service: &str, line: &str) {
    let source = production_source(relative);
    let needle = format!("\"{service}\"");
    assert!(
        !source.contains(&needle),
        "{relative} still hardcodes the legacy keychain service {service:?} \
         (see {line}). Only brand.rs may hold a legacy brand string; the \
         service name must come from brand::canonical() so a rename can \
         migrate the entries written under the old one."
    );
}

#[test]
#[should_panic(expected = "still hardcodes the legacy keychain service")]
fn desktop_keychain_service_is_not_frozen_into_a_literal() {
    assert_no_frozen_service_literal(
        "src/secure_storage.rs",
        brand::LEGACY.keychain_service,
        "src/secure_storage.rs:4",
    );
}

#[test]
#[should_panic(expected = "still hardcodes the legacy keychain service")]
fn ssh_keychain_service_is_not_frozen_into_a_literal() {
    assert_no_frozen_service_literal(
        "src/ssh/credential_store.rs",
        brand::LEGACY.keychain_ssh_service,
        "src/ssh/credential_store.rs:8",
    );
}

/// A compat read is not optional. Once the canonical service name changes,
/// every credential already on a user's machine lives under the legacy name;
/// a getter that only consults `canonical()` returns `NoEntry` for all of them.
fn assert_reads_fall_back_to_legacy(relative: &str, legacy_field: &str, loses: &str) {
    let source = production_source(relative);
    let needle = format!("LEGACY.{legacy_field}");
    assert!(
        source.contains(&needle),
        "{relative} never consults brand::{needle}, so after the rename every \
         lookup misses and the user silently loses {loses}. Reads must try the \
         canonical service and fall back to the legacy one."
    );
}

#[test]
#[should_panic(expected = "never consults brand::LEGACY.keychain_ssh_service")]
fn ssh_credential_reads_fall_back_to_the_legacy_service() {
    assert_reads_fall_back_to_legacy(
        "src/ssh/credential_store.rs",
        "keychain_ssh_service",
        "every SSH password and private-key passphrase",
    );
}

#[test]
#[should_panic(expected = "never consults brand::LEGACY.keychain_service")]
fn desktop_credential_reads_fall_back_to_the_legacy_service() {
    assert_reads_fall_back_to_legacy(
        "src/secure_storage.rs",
        "keychain_service",
        "every stored project environment variable",
    );
}
