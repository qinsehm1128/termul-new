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
//! # The seam these tests execute through (T-A00)
//!
//! Originally the behavioural test — pre-seed the legacy entries, inject the
//! canonical service names, assert the new names read the values while the
//! legacy entries survive — could not be written at all, for two independent
//! reasons:
//!
//! 1. **No reachable production seam.** `mod secure_storage` and `mod ssh` were
//!    private to `se_manager_lib` and neither was re-exported;
//!    `keyring_get`/`keyring_set` were additionally `pub(crate)`. Nothing under
//!    `tests/` could call the credential paths at all.
//!
//! 2. **No injection point.** Both modules called
//!    `keyring::Entry::new(SERVICE_NAME, key)` directly, so the backend was
//!    whatever the compile-time cargo feature selected — on macOS the user's
//!    real login keychain. `keyring::mock` cannot stand in: its builder reports
//!    `CredentialPersistence::EntryOnly` and hands every `Entry::new` a *fresh
//!    empty* credential, so a pre-seeded legacy keychain is not representable.
//!    `set_default_credential_builder` is process-global (`RwLock`), not
//!    thread-local, so it does not compose with the thread-local brand seam the
//!    way the rest of this harness relies on.
//!
//! `src/credentials.rs` closes both: it is a public façade every credential
//! read/write goes through, and its backend is injectable *per thread*, the same
//! shape as `brand::override_canonical`.
//! `production_reads_a_credential_seeded_under_the_legacy_service` is therefore
//! a real behavioural test — it calls production `keyring_get`,
//! `credential_store::get_password` and `get_passphrase` against a keychain that
//! only holds legacy-service entries.
//!
//! `fake_keychain_backend_holds_the_legacy_entries_the_canonical_service_cannot_see`
//! remains as the oracle underneath it: it proves, at the raw `keyring::Entry`
//! level, that a `(service, user)`-keyed store really does distinguish the two
//! services, so "the canonical service read it" is a statement about the
//! migration rather than about a store that collapsed the two names.

use std::any::Any;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use keyring::credential::{Credential, CredentialApi, CredentialBuilder, CredentialBuilderApi};
use keyring::{Entry, Error};
use serde_json::Value;

use se_manager_lib::brand::{self, BrandCanonical, DEFAULT_CANONICAL};
use se_manager_lib::credentials::{self, CredentialBackend, CredentialError};
use se_manager_lib::{keyring_delete, keyring_get, ssh_credential_store};

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

    fn peek(&self, service: &str, user: &str) -> Option<String> {
        self.secrets
            .lock()
            .unwrap()
            .get(&(service.to_string(), user.to_string()))
            .map(|secret| String::from_utf8(secret.clone()).expect("fixture secrets are UTF-8"))
    }
}

/// The same store, seen through the production façade instead of through
/// `keyring::Entry`. One type backs both so a test cannot accidentally compare
/// two different notions of "the keychain".
impl CredentialBackend for FixtureKeychain {
    fn get(&self, service: &str, key: &str) -> Result<Option<String>, CredentialError> {
        Ok(self.peek(service, key))
    }

    fn set(&self, service: &str, key: &str, value: &str) -> Result<(), CredentialError> {
        self.seed(service, key, value);
        Ok(())
    }

    fn delete(&self, service: &str, key: &str) -> Result<(), CredentialError> {
        self.secrets
            .lock()
            .unwrap()
            .remove(&(service.to_string(), key.to_string()));
        Ok(())
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
// The behavioural test the seam makes possible
// ---------------------------------------------------------------------------

/// Entries that are deliberately **absent** from the frozen fixture.
///
/// This is what makes the test below mutation-sensitive. `installed_keychain()`
/// has already pointed `keyring`'s process-global builder at the fixture store,
/// so an implementation that bypasses the façade and calls `Entry::new` still
/// reaches a fake keychain rather than the developer's login keychain — but it
/// reaches one that has never heard of these keys, and the assertions go red.
const INJECTED_PROJECT_KEY: &str =
    "project:5e2a9d31-64c8-4b17-9a03-2d7f81c4e6b9:env:AWS_SECRET_ACCESS_KEY";
const INJECTED_PROJECT_SECRET: &str = "legacy-service-project-secret";
const INJECTED_PROFILE_ID: &str = "7c0f2e45-1b93-48da-a6e7-5904d3b81fa2";
const INJECTED_SSH_PASSWORD: &str = "legacy-service-ssh-password";
const INJECTED_SSH_PASSPHRASE: &str = "legacy-service-ssh-passphrase";

/// A keychain holding nothing but pre-rename entries, injected through the
/// production façade.
fn keychain_with_only_legacy_entries() -> Arc<FixtureKeychain> {
    let store = Arc::new(FixtureKeychain::default());
    store.seed(
        brand::LEGACY.keychain_service,
        INJECTED_PROJECT_KEY,
        INJECTED_PROJECT_SECRET,
    );
    store.seed(
        brand::LEGACY.keychain_ssh_service,
        &format!("{INJECTED_PROFILE_ID}-password"),
        INJECTED_SSH_PASSWORD,
    );
    store.seed(
        brand::LEGACY.keychain_ssh_service,
        &format!("{INJECTED_PROFILE_ID}-passphrase"),
        INJECTED_SSH_PASSPHRASE,
    );
    store
}

/// The whole point of M-09 and M-10, executed against production code.
///
/// The canonical service names are the *post*-rename ones and the keychain
/// holds only pre-rename entries — exactly the state every existing install is
/// in the moment the rename ships. Production must still find all three
/// credentials, must copy them forward, and must leave the originals in place.
#[test]
fn production_reads_a_credential_seeded_under_the_legacy_service() {
    // Pin keyring's process-global builder to the fixture store first, so a
    // regression that bypasses the façade cannot reach a real login keychain.
    let _global = installed_keychain();

    let post = post_rename();
    let _brand = brand::override_canonical(post);
    assert_ne!(
        post.keychain_service, brand::LEGACY.keychain_service,
        "the post-rename injection must be a different spelling or this proves nothing"
    );

    let store = keychain_with_only_legacy_entries();
    let password_key = format!("{INJECTED_PROFILE_ID}-password");
    let passphrase_key = format!("{INJECTED_PROFILE_ID}-passphrase");
    assert_eq!(
        store.peek(post.keychain_service, INJECTED_PROJECT_KEY),
        None,
        "precondition: nothing has been written under the canonical service yet"
    );
    assert_eq!(
        store.peek(post.keychain_ssh_service, &password_key),
        None,
        "precondition: nothing has been written under the canonical SSH service yet"
    );

    let _backend = credentials::override_backend(Arc::clone(&store) as Arc<dyn CredentialBackend>);

    assert_eq!(
        keyring_get(INJECTED_PROJECT_KEY).expect("the project secret read succeeds"),
        Some(INJECTED_PROJECT_SECRET.to_string()),
        "M-09: a project environment secret written before the rename must still \
         be readable after it"
    );
    assert_eq!(
        ssh_credential_store::get_password(INJECTED_PROFILE_ID).expect("the SSH password read succeeds"),
        Some(INJECTED_SSH_PASSWORD.to_string()),
        "M-10: an SSH password written before the rename must still be readable"
    );
    assert_eq!(
        ssh_credential_store::get_passphrase(INJECTED_PROFILE_ID)
            .expect("the SSH passphrase read succeeds"),
        Some(INJECTED_SSH_PASSPHRASE.to_string()),
        "M-10: a private-key passphrase written before the rename must still be readable"
    );

    // Carried forward, so later reads no longer depend on the fallback.
    assert_eq!(
        store.peek(post.keychain_service, INJECTED_PROJECT_KEY),
        Some(INJECTED_PROJECT_SECRET.to_string())
    );
    assert_eq!(
        store.peek(post.keychain_ssh_service, &password_key),
        Some(INJECTED_SSH_PASSWORD.to_string())
    );
    assert_eq!(
        store.peek(post.keychain_ssh_service, &passphrase_key),
        Some(INJECTED_SSH_PASSPHRASE.to_string())
    );

    // And copied, not moved (FORBID-05): a user who downgrades still has them.
    assert!(
        store.contains(brand::LEGACY.keychain_service, INJECTED_PROJECT_KEY),
        "the legacy project entry must survive the migration"
    );
    assert!(
        store.contains(brand::LEGACY.keychain_ssh_service, &password_key),
        "the legacy SSH password entry must survive the migration"
    );
    assert!(
        store.contains(brand::LEGACY.keychain_ssh_service, &passphrase_key),
        "the legacy SSH passphrase entry must survive the migration"
    );
}

/// The same claim as the test above, with **no injection at all**.
///
/// T-A20 flipped `DEFAULT_CANONICAL`, so the service names in force here are the
/// ones this build actually ships to users. That distinction is the whole point:
/// the test above proves the *seam* carries credentials forward under whatever
/// names it is handed, and would keep passing even if the flip had never landed
/// or were reverted. This one proves the shipped values do it, which is the
/// state a user's machine is in the first time they launch the renamed app.
///
/// The `assert_ne!` pair is the precondition, not decoration: if the flip is
/// reverted, `canonical() == LEGACY`, both getters short-circuit before the
/// fallback ever runs, and everything below would pass without exercising a
/// single line of the compatibility read.
#[test]
fn the_shipped_service_names_still_read_entries_left_under_the_legacy_ones() {
    let _global = installed_keychain();

    assert_ne!(
        DEFAULT_CANONICAL.keychain_service,
        brand::LEGACY.keychain_service,
        "the desktop keychain service has not been flipped, so nothing below \
         exercises the compatibility read"
    );
    assert_ne!(
        DEFAULT_CANONICAL.keychain_ssh_service,
        brand::LEGACY.keychain_ssh_service,
        "the SSH keychain service has not been flipped, so nothing below \
         exercises the compatibility read"
    );

    let store = keychain_with_only_legacy_entries();
    let password_key = format!("{INJECTED_PROFILE_ID}-password");
    let passphrase_key = format!("{INJECTED_PROFILE_ID}-passphrase");
    assert_eq!(
        store.peek(DEFAULT_CANONICAL.keychain_service, INJECTED_PROJECT_KEY),
        None,
        "precondition: the keychain holds nothing but pre-rename entries"
    );

    let _backend = credentials::override_backend(Arc::clone(&store) as Arc<dyn CredentialBackend>);

    assert_eq!(
        keyring_get(INJECTED_PROJECT_KEY).expect("the project secret read succeeds"),
        Some(INJECTED_PROJECT_SECRET.to_string()),
        "a project environment secret written by the pre-rename build must still \
         be readable under the service name this build ships"
    );
    assert_eq!(
        ssh_credential_store::get_password(INJECTED_PROFILE_ID)
            .expect("the SSH password read succeeds"),
        Some(INJECTED_SSH_PASSWORD.to_string()),
        "an SSH password written by the pre-rename build must still be readable"
    );
    assert_eq!(
        ssh_credential_store::get_passphrase(INJECTED_PROFILE_ID)
            .expect("the SSH passphrase read succeeds"),
        Some(INJECTED_SSH_PASSPHRASE.to_string()),
        "a private-key passphrase written by the pre-rename build must still be readable"
    );

    // Carried forward under the shipped names …
    assert_eq!(
        store.peek(DEFAULT_CANONICAL.keychain_service, INJECTED_PROJECT_KEY),
        Some(INJECTED_PROJECT_SECRET.to_string())
    );
    assert_eq!(
        store.peek(DEFAULT_CANONICAL.keychain_ssh_service, &password_key),
        Some(INJECTED_SSH_PASSWORD.to_string())
    );
    assert_eq!(
        store.peek(DEFAULT_CANONICAL.keychain_ssh_service, &passphrase_key),
        Some(INJECTED_SSH_PASSPHRASE.to_string())
    );

    // … and copied, not moved (FORBID-05).
    assert!(
        store.contains(brand::LEGACY.keychain_service, INJECTED_PROJECT_KEY),
        "the legacy project entry must survive the compatibility read"
    );
    assert!(
        store.contains(brand::LEGACY.keychain_ssh_service, &password_key)
            && store.contains(brand::LEGACY.keychain_ssh_service, &passphrase_key),
        "the legacy SSH entries must survive the compatibility read"
    );
}

/// A user-initiated delete must purge the legacy service too, or the very next
/// read resurrects the credential the user just revoked.
///
/// This is the boundary of FORBID-05, executed. The rule binds the *migration*:
/// a compat read copies forward and never deletes, so it cannot destroy data the
/// user did not ask to lose — `production_reads_a_credential_seeded_under_the_legacy_service`
/// above asserts exactly that. A delete is the opposite case, and the two must
/// not be collapsed: leaving the pre-rename entry behind would mean "delete my
/// stored SSH password" silently did nothing.
#[test]
fn deleting_a_credential_purges_the_legacy_service_too() {
    let _global = installed_keychain();

    let post = post_rename();
    let _brand = brand::override_canonical(post);

    let store = keychain_with_only_legacy_entries();
    let password_key = format!("{INJECTED_PROFILE_ID}-password");
    let passphrase_key = format!("{INJECTED_PROFILE_ID}-passphrase");
    let _backend = credentials::override_backend(Arc::clone(&store) as Arc<dyn CredentialBackend>);

    // Read first, so the entries exist under *both* services — the realistic
    // state after a compat read, and the state in which deleting only the
    // canonical one looks like it worked.
    keyring_get(INJECTED_PROJECT_KEY).expect("the project secret is readable");
    ssh_credential_store::get_password(INJECTED_PROFILE_ID).expect("the SSH password is readable");
    ssh_credential_store::get_passphrase(INJECTED_PROFILE_ID)
        .expect("the SSH passphrase is readable");
    assert!(
        store.contains(post.keychain_service, INJECTED_PROJECT_KEY)
            && store.contains(brand::LEGACY.keychain_service, INJECTED_PROJECT_KEY),
        "precondition: the secret is now under both services"
    );

    keyring_delete(INJECTED_PROJECT_KEY).expect("deleting the project secret succeeds");
    ssh_credential_store::delete_credentials(INJECTED_PROFILE_ID)
        .expect("deleting the SSH credentials succeeds");

    // The assertion that matters: a subsequent read must not fall back to the
    // legacy service and hand the secret back.
    assert_eq!(
        keyring_get(INJECTED_PROJECT_KEY).expect("the read after delete succeeds"),
        None,
        "a deleted project environment secret must not be resurrected by the \
         legacy-service fallback"
    );
    assert_eq!(
        ssh_credential_store::get_password(INJECTED_PROFILE_ID)
            .expect("the read after delete succeeds"),
        None,
        "a deleted SSH password must not be resurrected by the legacy-service fallback"
    );
    assert_eq!(
        ssh_credential_store::get_passphrase(INJECTED_PROFILE_ID)
            .expect("the read after delete succeeds"),
        None,
        "a deleted private-key passphrase must not be resurrected by the \
         legacy-service fallback"
    );

    // And nothing is left under either spelling.
    for (service, key) in [
        (post.keychain_service, INJECTED_PROJECT_KEY),
        (brand::LEGACY.keychain_service, INJECTED_PROJECT_KEY),
        (post.keychain_ssh_service, password_key.as_str()),
        (brand::LEGACY.keychain_ssh_service, password_key.as_str()),
        (post.keychain_ssh_service, passphrase_key.as_str()),
        (brand::LEGACY.keychain_ssh_service, passphrase_key.as_str()),
    ] {
        assert!(
            !store.contains(service, key),
            "{key:?} still exists under {service:?} after the user deleted it"
        );
    }
}

/// The credential seam must be thread-local for the same reason the brand seam
/// is (`brand::tests::override_does_not_leak_into_other_threads`): cargo runs
/// test fns on parallel threads inside one process. A process-global injection —
/// which is exactly what `keyring::set_default_credential_builder` is, and why
/// it could not serve as this seam — would hand one test's fake keychain to
/// every sibling test and make every result here non-deterministic.
#[test]
fn an_injected_credential_backend_is_invisible_to_other_threads() {
    let _global = installed_keychain();

    let store = keychain_with_only_legacy_entries();
    let _backend = credentials::override_backend(Arc::clone(&store) as Arc<dyn CredentialBackend>);

    assert_eq!(
        keyring_get(INJECTED_PROJECT_KEY).expect("this thread reads the injected keychain"),
        Some(INJECTED_PROJECT_SECRET.to_string())
    );

    let observed = std::thread::spawn(|| keyring_get(INJECTED_PROJECT_KEY))
        .join()
        .expect("the sibling thread completes")
        .expect("the sibling thread's read succeeds");
    assert_eq!(
        observed, None,
        "a sibling thread must fall back to the shipped backend; if it saw the \
         injected keychain, every assertion in this harness would depend on \
         which test happened to run first"
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
fn desktop_keychain_service_is_not_frozen_into_a_literal() {
    assert_no_frozen_service_literal(
        "src/secure_storage.rs",
        brand::LEGACY.keychain_service,
        "src/secure_storage.rs:4",
    );
}

#[test]
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
fn ssh_credential_reads_fall_back_to_the_legacy_service() {
    assert_reads_fall_back_to_legacy(
        "src/ssh/credential_store.rs",
        "keychain_ssh_service",
        "every SSH password and private-key passphrase",
    );
}

#[test]
fn desktop_credential_reads_fall_back_to_the_legacy_service() {
    assert_reads_fall_back_to_legacy(
        "src/secure_storage.rs",
        "keychain_service",
        "every stored project environment variable",
    );
}
