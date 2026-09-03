//! Secure Credential Store
//!
//! Uses the OS keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service)
//! to store and retrieve SSH passwords and passphrases instead of persisting them in plain text.

use crate::brand;
use crate::credentials::{self, CredentialError};

/// Key suffix for password credentials.
///
/// `pub(crate)` because the brand-merge orchestrator has to *enumerate* the
/// keys a pre-rename install wrote (M-10), and a second copy of the suffix in
/// that module would silently stop matching the moment either one changed.
pub(crate) const PASSWORD_SUFFIX: &str = "password";
/// Key suffix for passphrase credentials. `pub(crate)` for the same reason as
/// [`PASSWORD_SUFFIX`].
pub(crate) const PASSPHRASE_SUFFIX: &str = "passphrase";

/// The keychain service this build writes to. Read through the brand seam on
/// the calling thread (FORBID-07) so a rename is a single edit in `brand.rs`
/// instead of a literal frozen into this file.
fn service() -> &'static str {
    brand::canonical().keychain_ssh_service
}

fn entry_error(error: &CredentialError, operation: &str) -> String {
    match error {
        CredentialError::Unavailable(message) => {
            format!("Failed to create keyring entry: {}", message)
        }
        CredentialError::Backend(message) => format!("{}: {}", operation, message),
    }
}

/// Read a credential from the current service, falling back to the service the
/// pre-rename app wrote to.
///
/// Without the fallback every SSH password and private-key passphrase on a
/// user's machine becomes invisible the moment the service name changes — the
/// profile simply stops authenticating and nothing names a cause. A value found
/// under the legacy service is copied forward; the legacy entry is never
/// deleted here (FORBID-05).
fn read_with_legacy_fallback(key: &str, operation: &str) -> Result<Option<String>, String> {
    let backend = credentials::backend();
    let canonical = service();

    if let Some(value) = backend
        .get(canonical, key)
        .map_err(|error| entry_error(&error, operation))?
    {
        return Ok(Some(value));
    }

    let legacy = brand::LEGACY.keychain_ssh_service;
    if legacy == canonical {
        return Ok(None);
    }
    let Some(value) = backend
        .get(legacy, key)
        .map_err(|error| entry_error(&error, operation))?
    else {
        return Ok(None);
    };
    // Best effort: failing to carry the credential forward must not fail the
    // read, it only means the next read falls back again.
    if let Err(error) = backend.set(canonical, key, &value) {
        log::warn!(
            "[SSH] Failed to carry a stored credential to the current keychain service: {}",
            entry_error(&error, "Failed to store credential in keychain")
        );
    }
    Ok(Some(value))
}

/// Store a password for the given profile ID in the OS keychain.
pub fn store_password(profile_id: &str, password: &str) -> Result<(), String> {
    let key = format!("{}-{}", profile_id, PASSWORD_SUFFIX);
    credentials::backend()
        .set(service(), &key, password)
        .map_err(|error| entry_error(&error, "Failed to store password in keychain"))
}

/// Retrieve a stored password for the given profile ID from the OS keychain.
pub fn get_password(profile_id: &str) -> Result<Option<String>, String> {
    let key = format!("{}-{}", profile_id, PASSWORD_SUFFIX);
    read_with_legacy_fallback(&key, "Failed to retrieve password from keychain")
}

/// Store a passphrase for the given profile ID in the OS keychain.
pub fn store_passphrase(profile_id: &str, passphrase: &str) -> Result<(), String> {
    let key = format!("{}-{}", profile_id, PASSPHRASE_SUFFIX);
    credentials::backend()
        .set(service(), &key, passphrase)
        .map_err(|error| entry_error(&error, "Failed to store passphrase in keychain"))
}

/// Retrieve a stored passphrase for the given profile ID from the OS keychain.
pub fn get_passphrase(profile_id: &str) -> Result<Option<String>, String> {
    let key = format!("{}-{}", profile_id, PASSPHRASE_SUFFIX);
    read_with_legacy_fallback(&key, "Failed to retrieve passphrase from keychain")
}

/// Delete a credential from the current service **and** `brand::LEGACY`.
///
/// Do not "fix" this back to deleting only the canonical entry. FORBID-05 binds
/// the *migration* path: `read_with_legacy_fallback` above copies and never
/// deletes, so a migration cannot destroy data the user did not ask to lose. A
/// user-initiated delete is the opposite case. Purging only the canonical entry
/// would let the very next `get_password` / `get_passphrase` fall back to the
/// legacy service and hand back an SSH password the user explicitly revoked.
///
/// Guarded by `tests/legacy_brand_keychain.rs::deleting_a_credential_purges_the_legacy_service_too`.
fn delete_key(profile_id: &str, suffix: &str, label: &str) -> Result<(), String> {
    let key = format!("{}-{}", profile_id, suffix);
    let backend = credentials::backend();
    let canonical = service();
    let operation = format!("Failed to delete {} from keychain", label);

    backend
        .delete(canonical, &key)
        .map_err(|error| entry_error(&error, &operation))?;

    let legacy = brand::LEGACY.keychain_ssh_service;
    if legacy != canonical {
        backend
            .delete(legacy, &key)
            .map_err(|error| entry_error(&error, &operation))?;
    }
    Ok(())
}

/// Delete the stored password for a profile.
pub fn delete_password(profile_id: &str) -> Result<(), String> {
    delete_key(profile_id, PASSWORD_SUFFIX, "password")
}

/// Delete the stored key passphrase for a profile.
pub fn delete_passphrase(profile_id: &str) -> Result<(), String> {
    delete_key(profile_id, PASSPHRASE_SUFFIX, "passphrase")
}

/// Delete all stored credentials for a profile (both password and passphrase).
pub fn delete_credentials(profile_id: &str) -> Result<(), String> {
    let mut errors = Vec::new();

    if let Err(e) = delete_password(profile_id) {
        errors.push(format!("password: {}", e));
    }

    if let Err(e) = delete_passphrase(profile_id) {
        errors.push(format!("passphrase: {}", e));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Failed to delete credentials: {}",
            errors.join("; ")
        ))
    }
}

/// Verify that the configured keyring backend actually persists secrets.
///
/// `keyring` selects its backend at compile time; without an OS-backend cargo
/// feature it silently uses an in-memory mock where `set_password` succeeds but
/// `get_password` returns `NoEntry`. This round-trips a throwaway entry and
/// returns an error if the store is non-functional, so misconfiguration is
/// caught at startup instead of silently losing every credential.
#[allow(dead_code)]
pub fn self_test() -> Result<(), String> {
    let key = format!("__selftest-{}", uuid::Uuid::new_v4());
    let probe = "ok";
    let backend = credentials::backend();
    let service = service();
    backend
        .set(service, &key, probe)
        .map_err(|e| format!("keyring write failed: {}", e))?;
    let read_back = match backend.get(service, &key) {
        Ok(Some(value)) => value,
        Ok(None) => {
            if let Err(del_err) = backend.delete(service, &key) {
                return Err(format!(
                    "keyring read-back found no entry and probe cleanup also failed: {}",
                    del_err
                ));
            }
            return Err(
                "keyring read-back found no entry (likely no OS backend compiled in)".to_string(),
            );
        }
        Err(e) => {
            if let Err(del_err) = backend.delete(service, &key) {
                return Err(format!(
                    "keyring read-back failed ({}) and probe cleanup also failed: {}",
                    e, del_err
                ));
            }
            return Err(format!(
                "keyring read-back failed (likely no OS backend compiled in): {}",
                e
            ));
        }
    };
    // Clean up the probe; a cleanup failure indicates a partially-working
    // backend and is itself worth surfacing.
    let delete_result = backend.delete(service, &key);
    if read_back != probe {
        return Err("keyring read-back mismatch (mock/in-memory store active)".to_string());
    }
    if let Err(del_err) = delete_result {
        return Err(format!("keyring probe cleanup failed: {}", del_err));
    }
    Ok(())
}
