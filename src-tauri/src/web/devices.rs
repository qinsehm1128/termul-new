//! Host-wide paired-device identity. The persisted file contains digests, never credentials.
use super::auth::{
    digest, generate_token, RemoteAccessAuthority, RemoteAuthError, RemotePrincipal,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;

pub const GRANTABLE_SCOPES: [&str; 8] = [
    "view",
    "terminal.input",
    "terminal.create",
    "agent.run",
    "files.write",
    "git.write",
    "screen.view",
    "screen.control",
];
const MAX_DEVICES: usize = 32;
const INACTIVE_SECONDS: u64 = 30 * 24 * 60 * 60;
pub const LEGACY_DEVICE_ID: &str = "legacy-host";
pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDevice {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub created_at: u64,
    pub last_seen: u64,
    pub scopes: BTreeSet<String>,
    pub revoked_at: Option<u64>,
    pub legacy: bool,
    pub project_id: Option<String>,
    pub connection_count: usize,
}
#[derive(Clone, Serialize, Deserialize)]
struct StoredDevice {
    device: RemoteDevice,
    credential_digest: [u8; 32],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuditEntry {
    pub at: u64,
    pub event: String,
    pub device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct DeviceRegistry {
    #[serde(default)]
    host_id: Option<String>,
    devices: Vec<StoredDevice>,
    // Tombstones retain revoked-credential recognition after a revoked row is archived.
    revoked_credentials: Vec<[u8; 32]>,
    #[serde(default)]
    host_fingerprint: Option<[u8; 32]>,
    pub(super) revision: u64,
    #[serde(default)]
    pub(super) legacy_revoked_at: Option<u64>,
    audit: Vec<DeviceAuditEntry>,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    pairing: Option<([u8; 32], u64)>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCode {
    pub code: String,
    pub expires_at: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairRequest {
    pub code: String,
    pub name: String,
    pub platform: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairResponse {
    pub host_id: String,
    pub device_id: String,
    pub device_token: String,
    pub scopes: BTreeSet<String>,
}
#[derive(Serialize)]
pub struct DeviceList {
    pub devices: Vec<RemoteDevice>,
    pub revision: u64,
}

impl DeviceRegistry {
    pub(super) fn load(path: PathBuf) -> Result<Self, RemoteAuthError> {
        let mut registry: Self = match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|_| RemoteAuthError::Provisioning)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(_) => return Err(RemoteAuthError::Provisioning),
        };
        if registry.devices.len() > MAX_DEVICES {
            return Err(RemoteAuthError::Provisioning);
        }
        registry.path = Some(path);
        if registry.host_id.is_none() {
            registry.host_id = Some(uuid::Uuid::new_v4().to_string());
            registry.revision = registry.revision.saturating_add(1);
        }
        registry.persist()?;
        Ok(registry)
    }
    fn persist(&self) -> Result<(), RemoteAuthError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path.parent().ok_or(RemoteAuthError::Provisioning)?;
        fs::create_dir_all(parent).map_err(|_| RemoteAuthError::Provisioning)?;
        let temporary = path.with_extension("json.tmp");
        let result = (|| -> std::io::Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        result.map_err(|_| RemoteAuthError::Provisioning)
    }
    fn record(&mut self, event: &str, id: &str, capability: Option<&str>) {
        self.audit.push(DeviceAuditEntry {
            at: now(),
            event: event.into(),
            device_id: id.into(),
            capability: capability.map(str::to_owned),
        });
        self.revision = self.revision.saturating_add(1);
        log::info!(target: "se_manager::web::devices", "operation={} device_id={} capability={}", event, id, capability.unwrap_or("-"));
    }
    fn expire(&mut self, at: u64) {
        let expired: Vec<_> = self
            .devices
            .iter_mut()
            .filter_map(|entry| {
                if entry.device.revoked_at.is_none()
                    && at.saturating_sub(entry.device.last_seen) >= INACTIVE_SECONDS
                {
                    entry.device.revoked_at = Some(at);
                    Some(entry.device.id.clone())
                } else {
                    None
                }
            })
            .collect();
        for id in expired {
            self.record("device_revoke", &id, None);
        }
    }
    pub(super) fn revoke_all(&mut self) {
        self.pairing = None;
        self.legacy_revoked_at = Some(now());
        self.record("device_revoke", LEGACY_DEVICE_ID, None);
        let ids: Vec<_> = self
            .devices
            .iter_mut()
            .filter_map(|entry| {
                if entry.device.revoked_at.is_none() {
                    entry.device.revoked_at = Some(now());
                    Some(entry.device.id.clone())
                } else {
                    None
                }
            })
            .collect();
        for id in ids {
            self.record("device_revoke", &id, None);
        }
    }
}
impl RemoteAccessAuthority {
    pub fn configure_device_store(&self, path: PathBuf) -> Result<(), RemoteAuthError> {
        let fingerprint = self.host_credential_fingerprint();
        let mut registry = DeviceRegistry::load(path)?;
        if let Some(fingerprint) = fingerprint {
            if registry.host_fingerprint != Some(fingerprint) {
                // A headless token/environment or persisted desktop credential
                // may have rotated while the process was stopped.
                if registry.host_fingerprint.is_some() || !registry.devices.is_empty() {
                    registry.revoke_all();
                }
                registry.host_fingerprint = Some(fingerprint);
                registry.legacy_revoked_at = None;
                registry.revision = registry.revision.saturating_add(1);
                registry.persist()?;
            }
        }
        *self.devices.lock() = registry;
        Ok(())
    }
    /// Public, non-secret profile identity; independent of bearer generations.
    pub fn host_id(&self) -> Result<String, RemoteAuthError> {
        self.change_devices(|registry| {
            if registry.host_id.is_none() {
                registry.host_id = Some(uuid::Uuid::new_v4().to_string());
                registry.revision = registry.revision.saturating_add(1);
            }
            Ok(registry.host_id.clone().expect("host identity initialized"))
        })
    }
    pub fn subscribe_devices(&self) -> tokio::sync::watch::Receiver<u64> {
        self.devices_tx.subscribe()
    }
    fn change_devices<T>(
        &self,
        change: impl FnOnce(&mut DeviceRegistry) -> Result<T, RemoteAuthError>,
    ) -> Result<T, RemoteAuthError> {
        let mut registry = self.devices.lock();
        let previous = registry.clone();
        registry.expire(now());
        let result = change(&mut registry);
        if registry.revision != previous.revision {
            if let Err(error) = registry.persist() {
                *registry = previous;
                return Err(error);
            }
            self.devices_tx.send_replace(registry.revision);
        }
        result
    }
    pub fn issue_pairing_code(&self) -> Result<PairingCode, RemoteAuthError> {
        if !self.generation_state().active {
            return Err(RemoteAuthError::Unconfigured);
        }
        let code = generate_token()?;
        let expires_at = now() + 60;
        self.devices.lock().pairing = Some((digest(code.as_bytes()), expires_at));
        Ok(PairingCode { code, expires_at })
    }
    pub fn pair_device(&self, request: PairRequest) -> Result<PairResponse, RemoteAuthError> {
        if !self.generation_state().active {
            return Err(RemoteAuthError::Unconfigured);
        }
        if request.name.trim().is_empty()
            || request.name.len() > 128
            || request.platform.len() > 64
            || request.platform.trim().is_empty()
        {
            return Err(RemoteAuthError::InvalidDevice);
        }
        self.change_devices(|registry| {
            let (expected, expires) = registry
                .pairing
                .ok_or(RemoteAuthError::InvalidPairingCode)?;
            if expires <= now() || !bool::from(expected.ct_eq(&digest(request.code.as_bytes()))) {
                return Err(RemoteAuthError::InvalidPairingCode);
            }
            if registry.devices.len() >= MAX_DEVICES {
                if let Some(index) = registry
                    .devices
                    .iter()
                    .position(|entry| entry.device.revoked_at.is_some())
                {
                    let archived = registry.devices.remove(index);
                    registry
                        .revoked_credentials
                        .push(archived.credential_digest);
                } else {
                    return Err(RemoteAuthError::DeviceLimit);
                }
            }
            let token = generate_token()?;
            let id = uuid::Uuid::new_v4().to_string();
            let scopes: BTreeSet<String> =
                GRANTABLE_SCOPES.into_iter().map(str::to_owned).collect();
            registry.devices.push(StoredDevice {
                device: RemoteDevice {
                    id: id.clone(),
                    name: request.name.trim().into(),
                    platform: request.platform.trim().into(),
                    created_at: now(),
                    last_seen: now(),
                    scopes: scopes.clone(),
                    revoked_at: None,
                    legacy: false,
                    project_id: None,
                    connection_count: 0,
                },
                credential_digest: digest(token.as_bytes()),
            });
            registry.pairing = None;
            registry.record("device_pair", &id, None);
            let host_id = registry
                .host_id
                .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                .clone();
            Ok(PairResponse {
                host_id,
                device_id: id,
                device_token: token,
                scopes,
            })
        })
    }
    pub(super) fn verify_device_digest(
        &self,
        candidate: [u8; 32],
    ) -> Result<String, RemoteAuthError> {
        self.change_devices(|registry| {
            if registry
                .revoked_credentials
                .iter()
                .any(|expected| bool::from(expected.ct_eq(&candidate)))
            {
                return Err(RemoteAuthError::DeviceRevoked);
            }
            let entry = registry
                .devices
                .iter_mut()
                .find(|entry| bool::from(entry.credential_digest.ct_eq(&candidate)))
                .ok_or(RemoteAuthError::InvalidCredential)?;
            if entry.device.revoked_at.is_some() {
                return Err(RemoteAuthError::DeviceRevoked);
            }
            let device_id = entry.device.id.clone();
            if entry.device.last_seen != now() {
                entry.device.last_seen = now();
                registry.revision = registry.revision.saturating_add(1);
            }
            Ok(device_id)
        })
    }
    pub fn validate_principal(&self, principal: &RemotePrincipal) -> Result<(), RemoteAuthError> {
        if !{
            let current = self.generation_state();
            current.active && current.generation == principal.generation()
        } {
            return Err(if principal.is_legacy() {
                RemoteAuthError::Forbidden
            } else {
                RemoteAuthError::DeviceRevoked
            });
        }
        if principal.is_legacy() {
            return if self.devices.lock().legacy_revoked_at.is_some() {
                Err(RemoteAuthError::DeviceRevoked)
            } else {
                Ok(())
            };
        }
        self.change_devices(|registry| {
            let entry = registry
                .devices
                .iter()
                .find(|entry| entry.device.id == principal.device_id())
                .ok_or(RemoteAuthError::DeviceRevoked)?;
            if entry.device.revoked_at.is_some() {
                Err(RemoteAuthError::DeviceRevoked)
            } else {
                Ok(())
            }
        })
    }
    pub fn require_scope(
        &self,
        principal: &RemotePrincipal,
        scope: &'static str,
    ) -> Result<(), RemoteAuthError> {
        self.validate_principal(principal)?;
        if principal.is_legacy() {
            return Ok(());
        }
        self.change_devices(|registry| {
            let granted = scope != "manage"
                && registry.devices.iter().any(|entry| {
                    entry.device.id == principal.device_id() && entry.device.scopes.contains(scope)
                });
            if granted {
                let at = now();
                if let Some(entry) = registry
                    .devices
                    .iter_mut()
                    .find(|entry| entry.device.id == principal.device_id())
                {
                    if entry.device.last_seen != at {
                        entry.device.last_seen = at;
                        registry.revision = registry.revision.saturating_add(1);
                    }
                }
                return Ok(());
            }
            registry.record("scope_denied", principal.device_id(), Some(scope));
            Err(RemoteAuthError::ScopeDenied(scope))
        })
    }
    pub(super) fn deny_scope(
        &self,
        principal: &RemotePrincipal,
        scope: &'static str,
    ) -> Result<(), RemoteAuthError> {
        self.change_devices(|registry| {
            registry.record("scope_denied", principal.device_id(), Some(scope));
            Err(RemoteAuthError::ScopeDenied(scope))
        })
    }
    pub fn attach_project(
        &self,
        principal: &RemotePrincipal,
        project_id: &str,
    ) -> Result<(), RemoteAuthError> {
        self.validate_principal(principal)?;
        if project_id.len() > 256 {
            return Err(RemoteAuthError::InvalidDevice);
        }
        self.change_devices(|registry| {
            if let Some(entry) = registry
                .devices
                .iter_mut()
                .find(|entry| entry.device.id == principal.device_id())
            {
                entry.device.project_id = Some(project_id.to_owned());
                entry.device.last_seen = now();
                registry.revision = registry.revision.saturating_add(1);
            }
            Ok(())
        })
    }
    pub fn list_devices(&self) -> Result<DeviceList, RemoteAuthError> {
        let active = self.generation_state().active;
        self.change_devices(|registry| {
            let mut devices: Vec<_> = registry
                .devices
                .iter()
                .map(|entry| entry.device.clone())
                .collect();
            if active {
                devices.push(RemoteDevice {
                    id: LEGACY_DEVICE_ID.into(),
                    name: "旧版主机凭据".into(),
                    platform: "legacy".into(),
                    created_at: 0,
                    last_seen: 0,
                    scopes: GRANTABLE_SCOPES.into_iter().map(str::to_owned).collect(),
                    revoked_at: registry.legacy_revoked_at,
                    legacy: true,
                    project_id: None,
                    connection_count: 0,
                });
            }
            Ok(DeviceList {
                devices,
                revision: registry.revision,
            })
        })
    }
    pub fn rename_device(&self, id: &str, name: &str) -> Result<(), RemoteAuthError> {
        if name.trim().is_empty() || name.len() > 128 {
            return Err(RemoteAuthError::InvalidDevice);
        }
        self.change_devices(|registry| {
            let entry = registry
                .devices
                .iter_mut()
                .find(|entry| entry.device.id == id)
                .ok_or(RemoteAuthError::InvalidDevice)?;
            entry.device.name = name.trim().into();
            registry.revision = registry.revision.saturating_add(1);
            Ok(())
        })
    }
    pub fn set_device_scopes(&self, id: &str, scopes: Vec<String>) -> Result<(), RemoteAuthError> {
        if scopes
            .iter()
            .any(|scope| !GRANTABLE_SCOPES.contains(&scope.as_str()))
        {
            return Err(RemoteAuthError::ScopeDenied("manage"));
        }
        self.change_devices(|registry| {
            let entry = registry
                .devices
                .iter_mut()
                .find(|entry| entry.device.id == id)
                .ok_or(RemoteAuthError::InvalidDevice)?;
            entry.device.scopes = scopes.into_iter().collect();
            registry.revision = registry.revision.saturating_add(1);
            Ok(())
        })
    }
    pub fn revoke_device(&self, id: &str) -> Result<(), RemoteAuthError> {
        self.change_devices(|registry| {
            let entry = registry
                .devices
                .iter_mut()
                .find(|entry| entry.device.id == id)
                .ok_or(RemoteAuthError::InvalidDevice)?;
            if entry.device.revoked_at.is_none() {
                entry.device.revoked_at = Some(now());
                registry.record("device_revoke", id, None);
            }
            Ok(())
        })
    }
    pub(super) fn rotate_device_credentials(
        &self,
        fingerprint: [u8; 32],
    ) -> Result<(), RemoteAuthError> {
        self.change_devices(|registry| {
            registry.revoke_all();
            registry.host_fingerprint = Some(fingerprint);
            registry.legacy_revoked_at = None;
            Ok(())
        })
    }
    pub fn revoke_all_devices(&self) -> Result<(), RemoteAuthError> {
        self.change_devices(|registry| {
            registry.revoke_all();
            Ok(())
        })
    }
    pub fn record_device_disconnect(&self, id: &str) -> Result<(), RemoteAuthError> {
        self.change_devices(|registry| {
            registry.record("device_disconnect", id, None);
            Ok(())
        })
    }
    pub fn device_audit(&self) -> Vec<DeviceAuditEntry> {
        self.devices.lock().audit.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::auth::{RemoteCapability, RemoteRouteClass};
    use axum::http::Method;

    fn authority() -> (std::sync::Arc<RemoteAccessAuthority>, String) {
        let authority = std::sync::Arc::new(RemoteAccessAuthority::desktop_memory());
        let (lease, _) = authority.adopt_or_issue_desktop_credential(None).unwrap();
        (authority, lease.bearer().to_owned())
    }
    fn pair(authority: &RemoteAccessAuthority) -> PairResponse {
        let code = authority.issue_pairing_code().unwrap();
        authority
            .pair_device(PairRequest {
                code: code.code,
                name: "Phone".into(),
                platform: "ios".into(),
            })
            .unwrap()
    }
    #[test]
    fn pairing_exchange_is_unique_and_code_is_single_use() {
        let (authority, _) = authority();
        let code = authority.issue_pairing_code().unwrap().code;
        let first = authority
            .pair_device(PairRequest {
                code: code.clone(),
                name: "Phone".into(),
                platform: "ios".into(),
            })
            .unwrap();
        assert_eq!(first.scopes.len(), 8);
        assert!(matches!(
            authority.pair_device(PairRequest {
                code,
                name: "Replay".into(),
                platform: "ios".into()
            }),
            Err(RemoteAuthError::InvalidPairingCode)
        ));
        let second = pair(&authority);
        assert_ne!(first.device_token, second.device_token);
        assert_ne!(first.device_id, second.device_id);
        assert_eq!(
            authority
                .verify_bearer(&first.device_token)
                .unwrap()
                .device_id(),
            first.device_id
        );
        assert!(!authority
            .verify_bearer(&first.device_token)
            .unwrap()
            .is_legacy());
    }
    #[test]
    fn expired_and_superseded_pair_codes_are_rejected() {
        let (authority, _) = authority();
        let old = authority.issue_pairing_code().unwrap();
        let new = authority.issue_pairing_code().unwrap();
        assert!(matches!(
            authority.pair_device(PairRequest {
                code: old.code,
                name: "Old".into(),
                platform: "ios".into()
            }),
            Err(RemoteAuthError::InvalidPairingCode)
        ));
        authority.devices.lock().pairing.as_mut().unwrap().1 = now() - 1;
        assert!(matches!(
            authority.pair_device(PairRequest {
                code: new.code,
                name: "Expired".into(),
                platform: "ios".into()
            }),
            Err(RemoteAuthError::InvalidPairingCode)
        ));
    }
    #[test]
    fn scopes_enforced_for_every_route_class_and_method() {
        let (authority, _) = authority();
        let paired = pair(&authority);
        let principal = authority.verify_bearer(&paired.device_token).unwrap();
        authority
            .set_device_scopes(&paired.device_id, vec![])
            .unwrap();
        let classes = [
            RemoteRouteClass::Project,
            RemoteRouteClass::Workspace,
            RemoteRouteClass::Conversation,
            RemoteRouteClass::ScheduledTask,
            RemoteRouteClass::CliSession,
            RemoteRouteClass::QuickTerminal,
            RemoteRouteClass::AcpWebSocket,
            RemoteRouteClass::TerminalWebSocket,
            RemoteRouteClass::FrontendLog,
            RemoteRouteClass::AcpCatalog,
            RemoteRouteClass::AcpInstall,
            RemoteRouteClass::Skill,
            RemoteRouteClass::Search,
            RemoteRouteClass::Mcp,
            RemoteRouteClass::MemoryIndex,
            RemoteRouteClass::Recovery,
            RemoteRouteClass::Git,
            RemoteRouteClass::Filesystem,
            RemoteRouteClass::Worktree,
            RemoteRouteClass::RemoteDesktop,
            RemoteRouteClass::RemoteDesktopWebSocket,
            RemoteRouteClass::RemoteAccess,
            RemoteRouteClass::AgentHooksStatus,
            RemoteRouteClass::AgentHooksManage,
            RemoteRouteClass::LocalOperator,
        ];
        for class in classes {
            for method in [Method::GET, Method::POST, Method::PATCH, Method::DELETE] {
                let required = class.required_scope(&method).unwrap();
                assert_eq!(
                    authority.authorize(&principal, RemoteCapability::Scope(required)),
                    Err(RemoteAuthError::ScopeDenied(required)),
                    "{} {}",
                    class.as_str(),
                    method
                );
                if required != "manage" {
                    authority
                        .set_device_scopes(&paired.device_id, vec![required.into()])
                        .unwrap();
                    assert!(authority
                        .authorize(&principal, RemoteCapability::Scope(required))
                        .is_ok());
                    authority
                        .set_device_scopes(&paired.device_id, vec![])
                        .unwrap();
                }
            }
        }
        assert_eq!(
            RemoteRouteClass::RemoteDesktop.required_scope(&Method::GET),
            Some("screen.view")
        );
        assert_eq!(
            RemoteRouteClass::RemoteDesktop.required_scope(&Method::POST),
            Some("screen.control")
        );
    }
    #[test]
    fn remote_manage_cannot_be_granted_even_with_all_scopes() {
        let (authority, _) = authority();
        let paired = pair(&authority);
        let principal = authority.verify_bearer(&paired.device_token).unwrap();
        assert_eq!(
            authority.require_scope(&principal, "manage"),
            Err(RemoteAuthError::ScopeDenied("manage"))
        );
        assert_eq!(
            authority.set_device_scopes(&paired.device_id, vec!["manage".into()]),
            Err(RemoteAuthError::ScopeDenied("manage"))
        );
    }
    #[test]
    fn legacy_host_bearer_keeps_all_scopes_and_rotation_revokes_all() {
        let (authority, legacy) = authority();
        let paired = pair(&authority);
        let principal = authority.verify_bearer(&paired.device_token).unwrap();
        let host = authority.verify_bearer(&legacy).unwrap();
        assert!(host.is_legacy());
        for scope in GRANTABLE_SCOPES.into_iter().chain(["manage"]) {
            assert!(authority.require_scope(&host, scope).is_ok());
        }
        let next = authority.rotate_desktop_credential().unwrap();
        assert!(authority.verify_bearer(&legacy).is_err());
        assert_eq!(
            authority.verify_bearer(&paired.device_token).unwrap_err(),
            RemoteAuthError::DeviceRevoked
        );
        assert_eq!(
            authority.validate_principal(&principal),
            Err(RemoteAuthError::DeviceRevoked)
        );
        assert!(authority.verify_bearer(next.bearer()).is_ok());
    }
    #[test]
    fn revoke_returns_device_revoked_not_rate_limited_or_generic_unauthorized() {
        let (authority, _) = authority();
        let paired = pair(&authority);
        let principal = authority.verify_bearer(&paired.device_token).unwrap();
        authority.revoke_device(&paired.device_id).unwrap();
        for _ in 0..20 {
            assert_eq!(
                authority
                    .verify_bearer_for_peer(&paired.device_token, "127.0.0.1".parse().unwrap()),
                Err(RemoteAuthError::DeviceRevoked)
            );
        }
        assert_eq!(
            authority.require_scope(&principal, "view"),
            Err(RemoteAuthError::DeviceRevoked)
        );
        assert_eq!(
            RemoteAuthError::DeviceRevoked.status(),
            axum::http::StatusCode::UNAUTHORIZED
        );
        assert_eq!(RemoteAuthError::DeviceRevoked.code(), "DEVICE_REVOKED");
    }
    #[test]
    fn thirty_days_inactive_revokes_and_preserves_audit() {
        let (authority, _) = authority();
        let paired = pair(&authority);
        authority.devices.lock().devices[0].device.last_seen = now() - INACTIVE_SECONDS;
        assert_eq!(
            authority.verify_bearer(&paired.device_token).unwrap_err(),
            RemoteAuthError::DeviceRevoked
        );
        let list = authority.list_devices().unwrap();
        assert!(list
            .devices
            .iter()
            .find(|device| device.id == paired.device_id)
            .unwrap()
            .revoked_at
            .is_some());
        assert!(authority
            .device_audit()
            .iter()
            .any(|entry| entry.device_id == paired.device_id && entry.event == "device_revoke"));
    }
    #[test]
    fn device_table_cap_is_32_and_revoked_slots_archive_without_losing_tombstones() {
        let (authority, _) = authority();
        let first = pair(&authority);
        for _ in 1..MAX_DEVICES {
            pair(&authority);
        }
        let code = authority.issue_pairing_code().unwrap().code;
        assert!(matches!(
            authority.pair_device(PairRequest {
                code,
                name: "33rd".into(),
                platform: "ios".into()
            }),
            Err(RemoteAuthError::DeviceLimit)
        ));
        authority.revoke_device(&first.device_id).unwrap();
        pair(&authority);
        assert_eq!(authority.devices.lock().devices.len(), MAX_DEVICES);
        assert_eq!(
            authority.verify_bearer(&first.device_token).unwrap_err(),
            RemoteAuthError::DeviceRevoked
        );
        assert!(authority
            .device_audit()
            .iter()
            .any(|entry| entry.device_id == first.device_id && entry.event == "device_revoke"));
    }
    #[test]
    fn disconnect_all_revokes_host_and_paired_but_allows_new_local_pairing() {
        let (authority, legacy) = authority();
        let paired = pair(&authority);
        let principal = authority.verify_bearer(&legacy).unwrap();
        authority.revoke_all_devices().unwrap();
        assert_eq!(
            authority.verify_bearer(&legacy).unwrap_err(),
            RemoteAuthError::DeviceRevoked
        );
        assert_eq!(
            authority.validate_principal(&principal),
            Err(RemoteAuthError::DeviceRevoked)
        );
        assert_eq!(
            authority.verify_bearer(&paired.device_token).unwrap_err(),
            RemoteAuthError::DeviceRevoked
        );
        let new = pair(&authority);
        assert!(authority.verify_bearer(&new.device_token).is_ok());
    }
    #[test]
    fn persistence_is_atomic_private_and_secret_free_with_durable_audit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let (authority, legacy) = authority();
        authority.configure_device_store(path.clone()).unwrap();
        let paired = pair(&authority);
        let contents = fs::read_to_string(&path).unwrap();
        assert!(!contents.contains(&paired.device_token));
        assert!(!contents.contains(&legacy));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let restored = RemoteAccessAuthority::desktop_memory();
        restored
            .adopt_or_issue_desktop_credential(Some(&legacy))
            .unwrap();
        restored.configure_device_store(path.clone()).unwrap();
        assert!(restored.verify_bearer(&paired.device_token).is_ok());
        restored.revoke_device(&paired.device_id).unwrap();
        let again = RemoteAccessAuthority::desktop_memory();
        again
            .adopt_or_issue_desktop_credential(Some(&legacy))
            .unwrap();
        again.configure_device_store(path).unwrap();
        assert_eq!(
            again.verify_bearer(&paired.device_token).unwrap_err(),
            RemoteAuthError::DeviceRevoked
        );
        assert!(again
            .device_audit()
            .iter()
            .any(|entry| entry.event == "device_revoke"));
    }
    #[test]
    fn pairing_rate_limit_is_bounded_per_peer_and_isolated_from_bearers() {
        let (authority, host) = authority();
        let peer = "192.0.2.10".parse().unwrap();
        for _ in 0..5 {
            assert!(matches!(
                authority.pair_device_for_peer(
                    PairRequest {
                        code: "wrong".into(),
                        name: "Phone".into(),
                        platform: "ios".into()
                    },
                    peer
                ),
                Err(RemoteAuthError::InvalidPairingCode)
            ));
        }
        assert!(matches!(
            authority.pair_device_for_peer(
                PairRequest {
                    code: "wrong".into(),
                    name: "Phone".into(),
                    platform: "ios".into()
                },
                peer
            ),
            Err(RemoteAuthError::RateLimited)
        ));
        let code = authority.issue_pairing_code().unwrap().code;
        assert!(matches!(
            authority.pair_device_for_peer(
                PairRequest {
                    code: code.clone(),
                    name: "Phone".into(),
                    platform: "ios".into()
                },
                peer
            ),
            Err(RemoteAuthError::RateLimited)
        ));
        assert!(authority
            .pair_device_for_peer(
                PairRequest {
                    code,
                    name: "Phone".into(),
                    platform: "ios".into()
                },
                "192.0.2.11".parse().unwrap()
            )
            .is_ok());
        assert!(authority.verify_bearer_for_peer(&host, peer).is_ok());
    }

    #[test]
    fn host_id_is_stable_across_restart_ip_changes_and_credential_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let (first, _) = authority();
        first.configure_device_store(path.clone()).unwrap();
        let id = first.host_id().unwrap();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_eq!(pair(&first).host_id, id);
        let rotated = first.rotate_desktop_credential().unwrap();
        assert_eq!(first.host_id().unwrap(), id);
        let restarted = RemoteAccessAuthority::desktop_memory();
        restarted
            .adopt_or_issue_desktop_credential(Some(rotated.bearer()))
            .unwrap();
        restarted.configure_device_store(path.clone()).unwrap();
        assert_eq!(restarted.host_id().unwrap(), id);
        assert_eq!(pair(&restarted).host_id, id);
        // A different origin/IP is not input to identity, nor is a new host key.
        let (new_key, _) = authority();
        new_key.configure_device_store(path).unwrap();
        assert_eq!(new_key.host_id().unwrap(), id);
    }

    #[test]
    fn changed_host_credential_on_restart_revokes_persisted_devices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let (first, _) = authority();
        first.configure_device_store(path.clone()).unwrap();
        let paired = pair(&first);
        let (next, host) = authority();
        next.configure_device_store(path).unwrap();
        assert_eq!(
            next.verify_bearer(&paired.device_token).unwrap_err(),
            RemoteAuthError::DeviceRevoked
        );
        assert!(next.verify_bearer(&host).is_ok());
        assert!(next
            .device_audit()
            .iter()
            .any(|entry| entry.event == "device_revoke" && entry.device_id == paired.device_id));
    }

    #[tokio::test]
    async fn denial_response_contains_capability_and_never_token() {
        let response =
            crate::web::auth::auth_error_response(RemoteAuthError::ScopeDenied("git.write"));
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"code":"FORBIDDEN","capability":"git.write"})
        );
    }
}
