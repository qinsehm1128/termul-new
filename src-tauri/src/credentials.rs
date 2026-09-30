//! The single seam through which host code reaches its credential store — a
//! user-only file since secrets left the OS keychain (see [`FileBackend`]).
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
//! Production never calls [`override_backend`]; it always sees the file at
//! [`default_credentials_path`].

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

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

/// Read/write access to one credential store, keyed by `(service, key)`.
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

/// The shipped backend: one JSON file under the user's workspace dir
/// (`~/.se-manager/credentials.json`), readable only by the user and shared by
/// every process of the app — GUI, Cores and the MCP gateway.
///
/// Not the OS keychain: a keychain entry is bound to the signature of the
/// binary that wrote it, so every update of an ad-hoc signed build made macOS
/// ask for the login password again, and a gateway waiting on that dialog left
/// every MCP server stuck connecting.
pub struct FileBackend {
    path: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CredentialFile {
    /// service → key → secret
    #[serde(default)]
    secrets: BTreeMap<String, BTreeMap<String, String>>,
}

impl FileBackend {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn read(&self) -> Result<CredentialFile, CredentialError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                CredentialError::Backend(format!("{}: {error}", self.path.display()))
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(CredentialFile::default()),
            Err(error) => Err(CredentialError::Backend(format!(
                "{}: {error}",
                self.path.display()
            ))),
        }
    }

    /// Read, change and replace the file while holding its lock, so writes
    /// from two processes cannot drop each other's entries. A file that does
    /// not parse is left alone rather than overwritten.
    fn update(
        &self,
        change: impl FnOnce(&mut CredentialFile) -> bool,
    ) -> Result<(), CredentialError> {
        let backend = |error: io::Error| {
            CredentialError::Backend(format!("{}: {error}", self.path.display()))
        };
        let parent = self.path.parent().ok_or_else(|| {
            CredentialError::Unavailable(format!("{} has no parent", self.path.display()))
        })?;
        fs::create_dir_all(parent).map_err(backend)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.path.with_extension("json.lock"))
            .map_err(backend)?;
        lock.lock().map_err(backend)?;

        let mut file = self.read()?;
        if !change(&mut file) {
            return Ok(());
        }
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|error| CredentialError::Backend(error.to_string()))?;
        write_private(&self.path, &bytes).map_err(backend)
    }
}

/// Write `bytes` to a new user-only file and rename it over `path`.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

impl CredentialBackend for FileBackend {
    fn get(&self, service: &str, key: &str) -> Result<Option<String>, CredentialError> {
        Ok(self
            .read()?
            .secrets
            .get(service)
            .and_then(|entries| entries.get(key))
            .cloned())
    }

    fn set(&self, service: &str, key: &str, value: &str) -> Result<(), CredentialError> {
        self.update(|file| {
            file.secrets
                .entry(service.to_owned())
                .or_default()
                .insert(key.to_owned(), value.to_owned());
            true
        })
    }

    fn delete(&self, service: &str, key: &str) -> Result<(), CredentialError> {
        self.update(|file| {
            let Some(entries) = file.secrets.get_mut(service) else {
                return false;
            };
            if entries.remove(key).is_none() {
                return false;
            }
            if entries.is_empty() {
                file.secrets.remove(service);
            }
            true
        })
    }
}

/// `~/<workspace dir>/credentials.json`, or under `SE_PROJECT_ROOT` when set.
/// Resolved on every call: it reads the brand seam on the caller's thread.
pub fn default_credentials_path() -> Option<PathBuf> {
    crate::mcp_core::config_root().map(|root| {
        root.join(crate::brand::canonical().workspace_dir)
            .join("credentials.json")
    })
}

struct ShippedBackend;

impl ShippedBackend {
    fn file() -> Result<FileBackend, CredentialError> {
        default_credentials_path()
            .map(FileBackend::at)
            .ok_or_else(|| CredentialError::Unavailable("no home directory".into()))
    }
}

impl CredentialBackend for ShippedBackend {
    fn get(&self, service: &str, key: &str) -> Result<Option<String>, CredentialError> {
        Self::file()?.get(service, key)
    }

    fn set(&self, service: &str, key: &str, value: &str) -> Result<(), CredentialError> {
        Self::file()?.set(service, key, value)
    }

    fn delete(&self, service: &str, key: &str) -> Result<(), CredentialError> {
        Self::file()?.delete(service, key)
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
    SHIPPED.get_or_init(|| Arc::new(ShippedBackend))
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
            backend
                .entries
                .lock()
                .unwrap()
                .insert((service.to_string(), key.to_string()), value.to_string());
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

    #[test]
    fn the_file_backend_keeps_secrets_per_service_for_every_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("credentials.json");
        let writer = FileBackend::at(&path);
        writer.set("svc", "k", "v1").unwrap();
        writer.set("svc", "k", "v2").unwrap();
        writer.set("other", "k", "o").unwrap();

        // Another process opens the same file.
        let reader = FileBackend::at(&path);
        assert_eq!(reader.get("svc", "k").unwrap().as_deref(), Some("v2"));
        assert_eq!(reader.get("other", "k").unwrap().as_deref(), Some("o"));
        assert_eq!(reader.get("svc", "missing").unwrap(), None);

        reader.delete("svc", "k").unwrap();
        reader.delete("svc", "k").unwrap();
        assert_eq!(writer.get("svc", "k").unwrap(), None);
        assert_eq!(writer.get("other", "k").unwrap().as_deref(), Some("o"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "got {mode:o}");
        }
    }

    #[test]
    fn a_missing_file_is_empty_and_a_corrupt_one_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let backend = FileBackend::at(&path);
        assert_eq!(backend.get("svc", "k").unwrap(), None);
        backend.delete("svc", "k").unwrap();
        assert!(!path.exists(), "deleting nothing must not create the file");

        fs::write(&path, b"{not json").unwrap();
        assert!(backend.get("svc", "k").is_err());
        assert!(backend.set("svc", "k", "v").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{not json");
    }

    #[test]
    fn concurrent_writers_do_not_drop_each_others_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let writers = (0..8)
            .map(|writer| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let backend = FileBackend::at(path);
                    for index in 0..20 {
                        backend
                            .set("svc", &format!("w{writer}-{index}"), "v")
                            .unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().unwrap();
        }
        let file: CredentialFile = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(file.secrets["svc"].len(), 160);
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
