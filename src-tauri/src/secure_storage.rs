use serde::{Deserialize, Serialize};

use crate::brand;
use crate::credentials::{self, CredentialError};

#[derive(Debug, Serialize, Deserialize)]
pub struct SecureStorageRequest {
    pub key: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SecureStorageSetRequest {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SecureStorageResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl<T> SecureStorageResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            code: None,
        }
    }

    pub fn error(error: String, code: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
            code: Some(code),
        }
    }
}

impl SecureStorageResponse<()> {
    pub fn success_void() -> Self {
        Self {
            success: true,
            data: None,
            error: None,
            code: None,
        }
    }
}

/// The keychain service this build writes to. Read through the brand seam on
/// the calling thread (FORBID-07) so a rename is a single edit in `brand.rs`
/// instead of a literal frozen into this file.
fn service() -> &'static str {
    brand::canonical().keychain_service
}

fn describe(error: &CredentialError, unavailable: &str, failed: &str) -> String {
    match error {
        // The message deliberately omits the backend's own text: it can name
        // the account whose secret was requested.
        CredentialError::Unavailable(_) => unavailable.to_string(),
        CredentialError::Backend(_) => failed.to_string(),
    }
}

/// Crate-private keyring helpers for host-owned credentials. Callers must not
/// log returned values, account names, or underlying backend errors.
///
/// Reads consult the canonical service first and fall back to
/// `brand::LEGACY.keychain_service`, because every secret already on a user's
/// machine was written under the legacy name; a getter that only asked the
/// canonical service would return `None` for all of them and the user would
/// watch their stored project environment variables go blank with no error.
/// A value found under the legacy service is copied forward so later reads are
/// direct — the legacy entry is never deleted (FORBID-05).
pub fn keyring_get(key: &str) -> Result<Option<String>, String> {
    let backend = credentials::backend();
    let canonical = service();

    let read = |from: &str| {
        backend.get(from, key).map_err(|error| {
            describe(
                &error,
                "keyring entry is unavailable",
                "keyring credential retrieval failed",
            )
        })
    };

    if let Some(value) = read(canonical)? {
        return Ok(Some(value));
    }

    let legacy = brand::LEGACY.keychain_service;
    if legacy == canonical {
        return Ok(None);
    }
    let Some(value) = read(legacy)? else {
        return Ok(None);
    };
    // Copy-forward is best effort: a failure here must not turn a successful
    // read into an error, it only means the next read falls back again.
    if let Err(error) = backend.set(canonical, key, &value) {
        log::warn!(
            "[secure-storage] failed to carry a credential to the current keychain service: {}",
            describe(
                &error,
                "keyring entry is unavailable",
                "keyring credential storage failed",
            )
        );
    }
    Ok(Some(value))
}

pub fn keyring_set(key: &str, value: &str) -> Result<(), String> {
    credentials::backend()
        .set(service(), key, value)
        .map_err(|error| {
            describe(
                &error,
                "keyring entry is unavailable",
                "keyring credential storage failed",
            )
        })
}

/// Deletes from both services: the user asked for the secret to be gone, and
/// leaving the legacy copy behind would resurrect it on the next fallback read.
/// This is the one path allowed to touch a legacy entry, because it is the user
/// deleting their own credential rather than a migration discarding data.
pub fn keyring_delete(key: &str) -> Result<(), String> {
    let backend = credentials::backend();
    let canonical = service();
    let describe_delete = |error: CredentialError| {
        describe(
            &error,
            "keyring entry is unavailable",
            "keyring credential deletion failed",
        )
    };

    backend.delete(canonical, key).map_err(describe_delete)?;

    let legacy = brand::LEGACY.keychain_service;
    if legacy != canonical {
        backend.delete(legacy, key).map_err(describe_delete)?;
    }
    Ok(())
}

#[tauri::command]
pub fn secure_storage_set(request: SecureStorageSetRequest) -> SecureStorageResponse<()> {
    match keyring_set(&request.key, &request.value) {
        Ok(()) => SecureStorageResponse::success_void(),
        Err(error) => SecureStorageResponse::error(error, "STORAGE_ERROR".to_string()),
    }
}

#[tauri::command]
pub fn secure_storage_get(request: SecureStorageRequest) -> SecureStorageResponse<String> {
    match keyring_get(&request.key) {
        Ok(Some(value)) => SecureStorageResponse::success(value),
        Ok(None) => SecureStorageResponse::error(
            format!("Secret not found for key: {}", request.key),
            "KEY_NOT_FOUND".to_string(),
        ),
        Err(error) => SecureStorageResponse::error(error, "RETRIEVAL_ERROR".to_string()),
    }
}

#[tauri::command]
pub fn secure_storage_delete(request: SecureStorageRequest) -> SecureStorageResponse<()> {
    match keyring_delete(&request.key) {
        Ok(()) => SecureStorageResponse::success_void(),
        Err(error) => SecureStorageResponse::error(error, "DELETE_ERROR".to_string()),
    }
}
