//! The single seam through which host code reaches an OS credential store.
//!
//! Every credential the app owns lives under a brand-bearing keychain *service*
//! name (`brand::canonical().keychain_service`,
//! `brand::canonical().keychain_ssh_service`). Renaming a service strands every
//! entry already written under the old one, so the rename has to be accompanied
//! by a compatibility read — and a compatibility read is only worth anything if
//! it can be *executed* in a test against a keychain that was pre-seeded under
//! the legacy service.
//!
//! That was not possible before this module existed, for two independent
//! reasons, both measured rather than assumed (see
//! `tests/legacy_brand_keychain.rs`):
//!
//! 1. `secure_storage` and `ssh::credential_store` each called
//!    `keyring::Entry::new(SERVICE_NAME, key)` directly, so the backend was
//!    whatever the compile-time cargo feature selected — on macOS the
//!    developer's real login keychain.
//! 2. `keyring::mock` cannot stand in: its builder reports
//!    `CredentialPersistence::EntryOnly` and discards the `(service, user)` pair
//!    entirely, so "a keychain holding an entry under the *old* service" is not
//!    even representable in it.
//!
//! So the backend is injectable here, and the injection is **thread-local**,
//! deliberately the same shape as [`crate::brand::override_canonical`]. Cargo
//! runs test fns on parallel threads inside one process; a process-global seam
//! would leak one test's fake store into every sibling test. `keyring`'s own
//! `set_default_credential_builder` is exactly such a process-global
//! (`RwLock`), which is why it cannot serve as this seam.
//!
//! Production never calls [`override_backend`]; it always sees
//! [`KeyringBackend`].

use std::cell::RefCell;
use std::fmt;
use std::sync::{Arc, OnceLock};

/// Why a credential operation could not be completed.
///
/// The two variants exist so callers can keep the distinct error messages they
/// had when they constructed a `keyring::Entry` themselves: building the handle
/// and using it were separate fallible steps.
#[derive(Debug, Clone)]
pub enum CredentialError {
    /// No handle could be obtained for `(service, key)`.
    Unavailable(String),
    /// The handle existed; the read/write/delete itself failed.
    Backend(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(message) | Self::Backend(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CredentialError {}

/// Read/write access to one OS credential store, keyed by `(service, key)`.
///
/// `service` is passed on every call rather than captured at construction
/// because a compatibility read has to consult two services — the canonical one
/// and `brand::LEGACY` — through the same backend.
pub trait CredentialBackend: Send + Sync {
    /// `Ok(None)` when the entry does not exist. Absence is not an error: it is
    /// the ordinary "this credential was never stored" answer.
    fn get(&self, service: &str, key: &str) -> Result<Option<String>, CredentialError>;

    /// Create or overwrite the entry.
    fn set(&self, service: &str, key: &str, value: &str) -> Result<(), CredentialError>;

    /// Remove the entry. Deleting an absent entry succeeds.
    fn delete(&self, service: &str, key: &str) -> Result<(), CredentialError>;
}

/// The shipped backend: `keyring::Entry` against whichever OS store the
/// compile-time cargo feature selected.
pub struct KeyringBackend;

impl KeyringBackend {
    fn entry(service: &str, key: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(service, key)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }
}

impl CredentialBackend for KeyringBackend {
    fn get(&self, service: &str, key: &str) -> Result<Option<String>, CredentialError> {
        match Self::entry(service, key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(CredentialError::Backend(error.to_string())),
        }
    }

    fn set(&self, service: &str, key: &str, value: &str) -> Result<(), CredentialError> {
        Self::entry(service, key)?
            .set_password(value)
            .map_err(|error| CredentialError::Backend(error.to_string()))
    }

    fn delete(&self, service: &str, key: &str) -> Result<(), CredentialError> {
        match Self::entry(service, key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(CredentialError::Backend(error.to_string())),
        }
    }
}

thread_local! {
    /// Per-thread override, for the same reason `brand.rs` uses one: cargo runs
    /// tests in parallel threads inside a single process, so a process-global
    /// seam would hand one test's fake store to every sibling test.
    static THREAD_OVERRIDE: RefCell<Option<Arc<dyn CredentialBackend>>> =
        const { RefCell::new(None) };
}

fn shipped_backend() -> &'static Arc<dyn CredentialBackend> {
    static SHIPPED: OnceLock<Arc<dyn CredentialBackend>> = OnceLock::new();
    SHIPPED.get_or_init(|| Arc::new(KeyringBackend))
}

/// The credential backend in force on **this thread** right now.
///
/// Always call this rather than caching the result — a cached handle freezes
/// before a test can override it, and (per FORBID-07) resolving it on a thread
/// other than the caller's silently yields the shipped backend.
pub fn backend() -> Arc<dyn CredentialBackend> {
    THREAD_OVERRIDE
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| Arc::clone(shipped_backend()))
}

/// Test seam: force a credential backend on **this thread** until the guard
/// drops. Production never calls this.
#[doc(hidden)]
#[must_use = "the override is reverted when the guard is dropped"]
pub fn override_backend(next: Arc<dyn CredentialBackend>) -> CredentialBackendGuard {
    let previous = THREAD_OVERRIDE.with(|slot| slot.replace(Some(next)));
    CredentialBackendGuard { previous }
}

/// Reverts an [`override_backend`] call when dropped.
#[doc(hidden)]
pub struct CredentialBackendGuard {
    previous: Option<Arc<dyn CredentialBackend>>,
}

impl Drop for CredentialBackendGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        THREAD_OVERRIDE.with(|slot| *slot.borrow_mut() = previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A `(service, key)`-keyed store — what a real OS keychain is.
    #[derive(Default)]
    struct MapBackend {
        entries: Mutex<BTreeMap<(String, String), String>>,
    }

    impl MapBackend {
        fn seeded(service: &str, key: &str, value: &str) -> Arc<Self> {
            let backend = Arc::new(Self::default());
            backend.entries.lock().unwrap().insert(
                (service.to_string(), key.to_string()),
                value.to_string(),
            );
            backend
        }
    }

    impl CredentialBackend for MapBackend {
        fn get(&self, service: &str, key: &str) -> Result<Option<String>, CredentialError> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .get(&(service.to_string(), key.to_string()))
                .cloned())
        }

        fn set(&self, service: &str, key: &str, value: &str) -> Result<(), CredentialError> {
            self.entries
                .lock()
                .unwrap()
                .insert((service.to_string(), key.to_string()), value.to_string());
            Ok(())
        }

        fn delete(&self, service: &str, key: &str) -> Result<(), CredentialError> {
            self.entries
                .lock()
                .unwrap()
                .remove(&(service.to_string(), key.to_string()));
            Ok(())
        }
    }

    #[test]
    fn override_replaces_the_backend_and_reverts_on_drop() {
        let injected = MapBackend::seeded("svc", "k", "v");
        {
            let _guard = override_backend(injected);
            assert_eq!(backend().get("svc", "k").unwrap().as_deref(), Some("v"));
        }
        // Back to the shipped backend, which knows nothing about "svc".
        assert!(THREAD_OVERRIDE.with(|slot| slot.borrow().is_none()));
    }

    #[test]
    fn the_backend_distinguishes_services_under_the_same_key() {
        let injected = MapBackend::seeded("legacy-service", "shared-key", "legacy value");
        let _guard = override_backend(injected);

        assert_eq!(
            backend().get("legacy-service", "shared-key").unwrap(),
            Some("legacy value".to_string())
        );
        assert_eq!(
            backend().get("canonical-service", "shared-key").unwrap(),
            None,
            "a keychain is keyed by (service, key); collapsing the service would \
             make a compatibility read meaningless"
        );
    }

    /// The whole reason this seam is thread-local rather than process-global.
    /// Same shape as `brand::tests::override_does_not_leak_into_other_threads`.
    #[test]
    fn override_does_not_leak_into_other_threads() {
        let _guard = override_backend(MapBackend::seeded("svc", "k", "v"));
        assert_eq!(backend().get("svc", "k").unwrap().as_deref(), Some("v"));

        let observed = std::thread::spawn(|| THREAD_OVERRIDE.with(|slot| slot.borrow().is_some()))
            .join()
            .unwrap();
        assert!(
            !observed,
            "a sibling thread must not see this thread's injected backend"
        );
    }
}
