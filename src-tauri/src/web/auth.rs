//! Shared remote-access authentication and authorization authority.
//!
//! The host injects one `Arc<RemoteAccessAuthority>` into the Axum router. Raw
//! credentials are accepted only at provisioning/verification boundaries; the
//! authority retains a SHA-256 digest and compares candidate digests in
//! constant time. Credentials, digests, authorization headers, URL fragments,
//! and recovery provenance must never be logged here.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::watch;
use url::Url;

const TOKEN_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 512;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
const FAILURE_LIMIT: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);
const FAILURE_STATE_TTL: Duration = Duration::from_secs(60);

/// Process-wide bound for retained unauthenticated failure state.
pub const MAX_AUTH_FAILURE_STATES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAuthoritySource {
    DesktopKeyring,
    OperatorTokenFile,
    Test,
    Unconfigured,
}

impl RemoteAuthoritySource {
    fn as_str(self) -> &'static str {
        match self {
            Self::DesktopKeyring => "desktop_keyring",
            Self::OperatorTokenFile => "operator_token_file",
            Self::Test => "test",
            Self::Unconfigured => "unconfigured",
        }
    }
}

/// Host-controlled request provenance. It is injected by the host/router and
/// is never derived from the reverse-proxy TCP peer or client headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IngressProvenance {
    LocalOperator,
    PublicTunnel,
}

impl IngressProvenance {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalOperator => "local_operator",
            Self::PublicTunnel => "public_tunnel",
        }
    }

    #[must_use]
    pub const fn allows_local_operator_mutation(self) -> bool {
        matches!(self, Self::LocalOperator)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteGenerationState {
    pub generation: u64,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationRetirementReceipt {
    pub generation: u64,
    pub credential_invalidated: bool,
    pub origins_cleared: bool,
    pub failure_state_cleared: bool,
    pub keyring_deleted: bool,
    /// True when keyring deletion failed and the host must retry the same generation.
    pub retry_owner: bool,
    pub stable_codes: Vec<&'static str>,
}

impl GenerationRetirementReceipt {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.stable_codes.is_empty() && !self.retry_owner
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteCapability {
    Connect,
    Read,
    Mutate,
    RecoveryInspect,
}

impl RemoteCapability {
    fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Read => "read",
            Self::Mutate => "mutate",
            Self::RecoveryInspect => "recovery_inspect",
        }
    }
}

/// Identifier-free route metadata attached by `web::router`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteRouteClass {
    Health,
    AcpWebSocket,
    TerminalWebSocket,
    Project,
    Mcp,
    Filesystem,
    Git,
    Search,
    Skill,
    CliSession,
    FrontendLog,
    Workspace,
    Conversation,
    ScheduledTask,
    Recovery,
    AcpCatalog,
    AcpInstall,
    Worktree,
}

impl RemoteRouteClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::AcpWebSocket => "acp_ws",
            Self::TerminalWebSocket => "terminal_ws",
            Self::Project => "project",
            Self::Mcp => "mcp",
            Self::Filesystem => "filesystem",
            Self::Git => "git",
            Self::Search => "search",
            Self::Skill => "skill",
            Self::CliSession => "cli_session",
            Self::FrontendLog => "frontend_log",
            Self::Workspace => "workspace",
            Self::Conversation => "conversation",
            Self::ScheduledTask => "scheduled_task",
            Self::Recovery => "recovery",
            Self::AcpCatalog => "acp_catalog",
            Self::AcpInstall => "acp_install",
            Self::Worktree => "worktree",
        }
    }

    fn capability(self, method: &Method) -> Option<RemoteCapability> {
        match self {
            Self::Health => None,
            Self::AcpWebSocket | Self::TerminalWebSocket => Some(RemoteCapability::Connect),
            Self::Recovery => Some(RemoteCapability::RecoveryInspect),
            _ if *method == Method::GET => Some(RemoteCapability::Read),
            _ => Some(RemoteCapability::Mutate),
        }
    }

    fn requires_http_bearer(self) -> bool {
        !matches!(
            self,
            Self::Health | Self::AcpWebSocket | Self::TerminalWebSocket
        )
    }

    /// Compatibility fallback for focused routers outside `web::router`.
    /// Production routes attach the enum directly and never log this path.
    fn from_path(path: &str) -> Option<Self> {
        if path == "/health" {
            Some(Self::Health)
        } else if path == "/ws" {
            Some(Self::AcpWebSocket)
        } else if path == "/terminal/ws" {
            Some(Self::TerminalWebSocket)
        } else if path == "/projects" || path.starts_with("/projects/") {
            Some(Self::Project)
        } else if path == "/mcp-servers" || path.starts_with("/mcp-servers/") {
            Some(Self::Mcp)
        } else if path.starts_with("/fs/") || path == "/shells" {
            Some(Self::Filesystem)
        } else if path.starts_with("/git/") {
            Some(Self::Git)
        } else if path.starts_with("/search/") {
            Some(Self::Search)
        } else if path == "/skills" || path.starts_with("/skills/") {
            Some(Self::Skill)
        } else if path == "/cli-sessions" || path.starts_with("/cli-sessions/") {
            Some(Self::CliSession)
        } else if path.starts_with("/log/") {
            Some(Self::FrontendLog)
        } else if path.starts_with("/workspace/") {
            Some(Self::Workspace)
        } else if path.starts_with("/conversation-recovery/") {
            Some(Self::Recovery)
        } else if path == "/conversations" || path.starts_with("/conversations/") {
            Some(Self::Conversation)
        } else if path == "/scheduled-tasks" || path.starts_with("/scheduled-tasks/") {
            Some(Self::ScheduledTask)
        } else if path == "/acp/catalog" || path.starts_with("/acp/catalog/") {
            Some(Self::AcpCatalog)
        } else if path == "/acp/install" {
            Some(Self::AcpInstall)
        } else if path.starts_with("/worktree/") {
            Some(Self::Worktree)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePrincipal {
    authority_source: RemoteAuthoritySource,
    generation: u64,
}

impl RemotePrincipal {
    #[must_use]
    pub fn authority_source(&self) -> RemoteAuthoritySource {
        self.authority_source
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(crate) fn for_tests(generation: u64) -> Self {
        Self {
            authority_source: RemoteAuthoritySource::Test,
            generation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAuthError {
    Unconfigured,
    InvalidCredential,
    InvalidOrigin,
    RateLimited,
    Forbidden,
    Provisioning,
}

impl RemoteAuthError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unconfigured | Self::Provisioning => "AUTH_CONFIGURATION_ERROR",
            Self::InvalidCredential => "UNAUTHORIZED",
            Self::InvalidOrigin | Self::Forbidden => "FORBIDDEN",
            Self::RateLimited => "RATE_LIMITED",
        }
    }

    #[must_use]
    pub const fn status(self) -> StatusCode {
        match self {
            Self::InvalidCredential => StatusCode::UNAUTHORIZED,
            Self::InvalidOrigin | Self::Forbidden => StatusCode::FORBIDDEN,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Unconfigured | Self::Provisioning => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn safe_message(self) -> &'static str {
        match self {
            Self::Unconfigured | Self::Provisioning => {
                "remote access authentication is not configured"
            }
            Self::InvalidCredential => "remote access credential is missing or invalid",
            Self::InvalidOrigin => "request Origin is missing or not allowed",
            Self::RateLimited => "too many failed authentication attempts",
            Self::Forbidden => "remote principal lacks the required capability",
        }
    }
}

impl std::fmt::Display for RemoteAuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.safe_message())
    }
}

impl std::error::Error for RemoteAuthError {}

/// One desktop credential generation. The host owns this lease and the raw
/// bearer; the authority retains only the generation metadata and digest.
pub struct DesktopCredentialLease {
    generation: u64,
    bearer: String,
}

impl DesktopCredentialLease {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub(crate) fn bearer(&self) -> &str {
        &self.bearer
    }
}

struct CredentialState {
    generation: u64,
    digest: Option<[u8; 32]>,
    source: RemoteAuthoritySource,
}

/// Distinguishes a real client address from traffic that terminated on loopback
/// (cloudflared / frpc / ssh -R). Those peers all appear as 127.0.0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FailureIdentity {
    Direct(IpAddr),
    Proxied,
}

fn failure_identity(peer: IpAddr) -> FailureIdentity {
    if peer.is_loopback() {
        FailureIdentity::Proxied
    } else {
        FailureIdentity::Direct(peer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FailureKey {
    identity: FailureIdentity,
    generation: u64,
}

#[derive(Debug, Clone)]
struct FailureState {
    window_started: Instant,
    failures: u32,
    locked_until: Option<Instant>,
    last_touched: Instant,
    lru_order: u64,
}

#[derive(Default)]
struct FailureLimiter {
    states: HashMap<FailureKey, FailureState>,
    next_lru_order: u64,
}

impl FailureLimiter {
    fn prune_expired(&mut self, now: Instant) {
        self.states.retain(|_, state| {
            now.saturating_duration_since(state.last_touched) < FAILURE_STATE_TTL
        });
    }

    fn next_lru_order(&mut self) -> u64 {
        self.next_lru_order = self.next_lru_order.saturating_add(1);
        self.next_lru_order
    }

    fn evict_lru_if_full(&mut self) {
        if self.states.len() < MAX_AUTH_FAILURE_STATES {
            return;
        }
        if let Some(oldest) = self
            .states
            .iter()
            .min_by_key(|(_, state)| state.lru_order)
            .map(|(key, _)| *key)
        {
            self.states.remove(&oldest);
        }
    }

    fn record_failure(&mut self, key: FailureKey, now: Instant) -> RemoteAuthError {
        self.prune_expired(now);
        if !self.states.contains_key(&key) {
            self.evict_lru_if_full();
        }
        let lru_order = self.next_lru_order();
        let state = self.states.entry(key).or_insert(FailureState {
            window_started: now,
            failures: 0,
            locked_until: None,
            last_touched: now,
            lru_order,
        });
        state.last_touched = now;
        state.lru_order = lru_order;
        if now.saturating_duration_since(state.window_started) >= FAILURE_WINDOW {
            state.window_started = now;
            state.failures = 0;
            state.locked_until = None;
        }
        if state.locked_until.is_some_and(|until| until > now) {
            return RemoteAuthError::RateLimited;
        }
        state.failures = state.failures.saturating_add(1);
        if state.failures > FAILURE_LIMIT {
            state.locked_until = Some(now + LOCKOUT);
            RemoteAuthError::RateLimited
        } else {
            RemoteAuthError::InvalidCredential
        }
    }

    fn len_at(&mut self, now: Instant) -> usize {
        self.prune_expired(now);
        self.states.len()
    }
}

/// Host-owned remote-access credential and policy authority.
pub struct RemoteAccessAuthority {
    credential: RwLock<CredentialState>,
    allowed_origins: RwLock<HashSet<String>>,
    failures: Mutex<FailureLimiter>,
    ingress: RwLock<IngressProvenance>,
    generation_tx: watch::Sender<RemoteGenerationState>,
}

impl std::fmt::Debug for RemoteAccessAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let credential = self.credential.read();
        formatter
            .debug_struct("RemoteAccessAuthority")
            .field("source", &credential.source)
            .field("generation", &credential.generation)
            .field("configured", &credential.digest.is_some())
            .field("allowed_origin_count", &self.allowed_origins.read().len())
            .field("ingress", &*self.ingress.read())
            .finish_non_exhaustive()
    }
}

impl RemoteAccessAuthority {
    #[must_use]
    pub fn unconfigured() -> Self {
        let (generation_tx, _generation_rx) = watch::channel(RemoteGenerationState {
            generation: 0,
            active: false,
        });
        Self {
            credential: RwLock::new(CredentialState {
                generation: 0,
                digest: None,
                source: RemoteAuthoritySource::Unconfigured,
            }),
            allowed_origins: RwLock::new(HashSet::new()),
            failures: Mutex::new(FailureLimiter::default()),
            ingress: RwLock::new(IngressProvenance::LocalOperator),
            generation_tx,
        }
    }

    /// Desktop pairing credentials persist in Preferences → Remote Access
    /// (`secrets.json`). Digest is empty until
    /// [`Self::adopt_or_issue_desktop_credential`].
    pub fn desktop_memory() -> Self {
        let (generation_tx, _generation_rx) = watch::channel(RemoteGenerationState {
            generation: 0,
            active: false,
        });
        Self {
            credential: RwLock::new(CredentialState {
                generation: 0,
                digest: None,
                source: RemoteAuthoritySource::DesktopKeyring,
            }),
            allowed_origins: RwLock::new(HashSet::new()),
            failures: Mutex::new(FailureLimiter::default()),
            ingress: RwLock::new(IngressProvenance::LocalOperator),
            generation_tx,
        }
    }

    /// Load a standalone credential from one securely opened operator-owned
    /// handle. Metadata/owner/DACL checks and bounded reads are performed on
    /// that exact handle; the pathname is never reopened after validation.
    pub fn from_token_file(path: &Path) -> Result<Self, RemoteAuthError> {
        let mut file = open_validated_token_file(path).inspect_err(|error| {
            log::error!(
                target: "se_manager::web::auth",
                "operation=token_file_open authority_source={} stable_code={}",
                RemoteAuthoritySource::OperatorTokenFile.as_str(),
                error.code()
            );
        })?;
        let token = read_token_from_validated_handle(&mut file).inspect_err(|error| {
            log::error!(
                target: "se_manager::web::auth",
                "operation=token_file_read authority_source={} stable_code={}",
                RemoteAuthoritySource::OperatorTokenFile.as_str(),
                error.code()
            );
        })?;
        let authority = Self::from_token(&token, RemoteAuthoritySource::OperatorTokenFile);
        log::info!(
            target: "se_manager::web::auth",
            "operation=authority_ready authority_source={} stable_code=OK",
            RemoteAuthoritySource::OperatorTokenFile.as_str()
        );
        Ok(authority)
    }

    fn from_token(token: &str, source: RemoteAuthoritySource) -> Self {
        let (generation_tx, _generation_rx) = watch::channel(RemoteGenerationState {
            generation: 1,
            active: true,
        });
        Self {
            credential: RwLock::new(CredentialState {
                generation: 1,
                digest: Some(digest(token.as_bytes())),
                source,
            }),
            allowed_origins: RwLock::new(HashSet::new()),
            failures: Mutex::new(FailureLimiter::default()),
            ingress: RwLock::new(IngressProvenance::LocalOperator),
            generation_tx,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(token: &str) -> Self {
        Self::from_token(token, RemoteAuthoritySource::Test)
    }

    pub fn set_ingress_provenance(&self, provenance: IngressProvenance) {
        *self.ingress.write() = provenance;
    }

    #[must_use]
    pub fn ingress_provenance(&self) -> IngressProvenance {
        *self.ingress.read()
    }

    #[must_use]
    pub fn subscribe_generation(&self) -> watch::Receiver<RemoteGenerationState> {
        self.generation_tx.subscribe()
    }

    #[must_use]
    pub fn generation_state(&self) -> RemoteGenerationState {
        let credential = self.credential.read();
        RemoteGenerationState {
            generation: credential.generation,
            active: credential.digest.is_some(),
        }
    }

    /// Generate and install a fresh desktop bearer generation. The raw bearer
    /// is returned exactly once in the host-owned lease; only its digest and
    /// monotonically increasing generation remain in the authority.
    pub fn rotate_desktop_credential(&self) -> Result<DesktopCredentialLease, RemoteAuthError> {
        let bearer = generate_token().inspect_err(|error| {
            log::error!(
                target: "se_manager::web::auth",
                "operation=rotate_credential stable_code={}",
                error.code()
            );
        })?;
        let mut credential = self.credential.write();
        let generation = credential
            .generation
            .checked_add(1)
            .ok_or(RemoteAuthError::Provisioning)?;
        match credential.source {
            RemoteAuthoritySource::DesktopKeyring | RemoteAuthoritySource::Test => {}
            RemoteAuthoritySource::OperatorTokenFile | RemoteAuthoritySource::Unconfigured => {
                return Err(RemoteAuthError::Provisioning);
            }
        }
        credential.generation = generation;
        credential.digest = Some(digest(bearer.as_bytes()));
        let source = credential.source;
        drop(credential);
        self.allowed_origins.write().clear();
        *self.failures.lock() = FailureLimiter::default();
        let _ = self.generation_tx.send(RemoteGenerationState {
            generation,
            active: true,
        });
        log::info!(
            target: "se_manager::web::auth",
            "operation=credential_rotate authority_source={} generation={} lifecycle_phase=rotate stable_code=OK",
            source.as_str(),
            generation
        );
        Ok(DesktopCredentialLease { generation, bearer })
    }

    /// Reuse the settings-file pairing bearer when present; otherwise issue a
    /// new generation. `issued` is true only when this call minted a new token.
    pub fn adopt_or_issue_desktop_credential(
        &self,
        stored_bearer: Option<&str>,
    ) -> Result<(DesktopCredentialLease, bool), RemoteAuthError> {
        let source = self.credential.read().source;
        match source {
            RemoteAuthoritySource::Test => {
                return Ok((self.rotate_desktop_credential()?, true));
            }
            RemoteAuthoritySource::DesktopKeyring => {}
            RemoteAuthoritySource::OperatorTokenFile | RemoteAuthoritySource::Unconfigured => {
                return Err(RemoteAuthError::Provisioning);
            }
        }
        if let Some(token) = stored_bearer {
            if validate_token(token).is_ok() {
                let candidate = digest(token.as_bytes());
                let (generation, changed) = {
                    let mut credential = self.credential.write();
                    if credential.digest == Some(candidate) {
                        (credential.generation, false)
                    } else {
                        let generation = if credential.generation == 0 {
                            1
                        } else {
                            credential
                                .generation
                                .checked_add(1)
                                .ok_or(RemoteAuthError::Provisioning)?
                        };
                        credential.generation = generation;
                        credential.digest = Some(candidate);
                        (generation, true)
                    }
                };
                if changed {
                    let _ = self.generation_tx.send(RemoteGenerationState {
                        generation,
                        active: true,
                    });
                }
                log::info!(
                    target: "se_manager::web::auth",
                    "operation=credential_adopt authority_source={} generation={} issued=false stable_code=OK",
                    RemoteAuthoritySource::DesktopKeyring.as_str(),
                    generation
                );
                return Ok((
                    DesktopCredentialLease {
                        generation,
                        bearer: token.to_string(),
                    },
                    false,
                ));
            }
        }
        Ok((self.rotate_desktop_credential()?, true))
    }

    /// Invalidate only the named generation. A stale compensation path cannot
    /// clear a newer generation that won a lifecycle race.
    pub fn invalidate_generation(&self, generation: u64) {
        let invalidated = {
            let mut credential = self.credential.write();
            if credential.generation == generation && credential.digest.is_some() {
                credential.digest = None;
                true
            } else {
                false
            }
        };
        if invalidated {
            self.allowed_origins.write().clear();
            *self.failures.lock() = FailureLimiter::default();
            let _ = self.generation_tx.send(RemoteGenerationState {
                generation,
                active: false,
            });
            log::warn!(
                target: "se_manager::web::auth",
                "operation=credential_invalidate generation={} lifecycle_phase=invalidate stable_code=GENERATION_INVALIDATED",
                generation
            );
        }
    }

    /// Retire a desktop generation completely. Digest, Origins, failure state,
    /// and generation observers are cleared; pairing persistence is owned by
    /// the host settings file, not the OS keyring.
    #[must_use]
    pub fn retire_generation(&self, generation: u64) -> GenerationRetirementReceipt {
        let (generation_matched, credential_invalidated) = {
            let mut credential = self.credential.write();
            if credential.generation == generation {
                (true, credential.digest.take().is_some())
            } else {
                (false, false)
            }
        };
        let origins_cleared = {
            let mut origins = self.allowed_origins.write();
            origins.clear();
            generation_matched
        };
        *self.failures.lock() = FailureLimiter::default();
        let failure_state_cleared = generation_matched;
        if credential_invalidated {
            let _ = self.generation_tx.send(RemoteGenerationState {
                generation,
                active: false,
            });
        }

        log::info!(
            target: "se_manager::web::auth",
            "operation=generation_retire generation={} lifecycle_phase=retire stable_code=OK keyring_deleted=true",
            generation
        );
        GenerationRetirementReceipt {
            generation,
            credential_invalidated,
            origins_cleared,
            failure_state_cleared,
            keyring_deleted: true,
            retry_owner: false,
            stable_codes: Vec::new(),
        }
    }

    /// Replace the credential digest without retaining the raw credential.
    pub fn install_credential(
        &self,
        token: &str,
        source: RemoteAuthoritySource,
    ) -> Result<(), RemoteAuthError> {
        validate_token(token)?;
        let mut credential = self.credential.write();
        credential.generation = credential
            .generation
            .checked_add(1)
            .ok_or(RemoteAuthError::Provisioning)?;
        credential.digest = Some(digest(token.as_bytes()));
        credential.source = source;
        let generation = credential.generation;
        drop(credential);
        *self.failures.lock() = FailureLimiter::default();
        let _ = self.generation_tx.send(RemoteGenerationState {
            generation,
            active: true,
        });
        Ok(())
    }

    pub fn set_public_origin(&self, origin: Url) -> Result<(), RemoteAuthError> {
        let normalized = normalize_origin(&origin)?;
        self.allowed_origins.write().insert(normalized);
        let generation = self.credential.read().generation;
        log::info!(
            target: "se_manager::web::auth",
            "operation=origin_register generation={} lifecycle_phase=register_origin stable_code=OK",
            generation
        );
        Ok(())
    }

    pub fn set_allowed_origins<I>(&self, origins: I) -> Result<(), RemoteAuthError>
    where
        I: IntoIterator<Item = Url>,
    {
        let normalized = origins
            .into_iter()
            .map(|origin| normalize_origin(&origin))
            .collect::<Result<HashSet<_>, _>>()?;
        if normalized.is_empty() {
            return Err(RemoteAuthError::InvalidOrigin);
        }
        *self.allowed_origins.write() = normalized;
        Ok(())
    }

    pub fn verify_bearer(&self, token: &str) -> Result<RemotePrincipal, RemoteAuthError> {
        validate_token(token).map_err(|_| RemoteAuthError::InvalidCredential)?;
        let credential = self.credential.read();
        let expected = match credential.digest {
            Some(expected) => expected,
            None if credential.generation > 0
                && credential.source != RemoteAuthoritySource::Unconfigured =>
            {
                return Err(RemoteAuthError::InvalidCredential);
            }
            None => return Err(RemoteAuthError::Unconfigured),
        };
        let candidate = digest(token.as_bytes());
        if bool::from(expected.ct_eq(&candidate)) {
            Ok(RemotePrincipal {
                authority_source: credential.source,
                generation: credential.generation,
            })
        } else {
            Err(RemoteAuthError::InvalidCredential)
        }
    }

    pub fn verify_ws_auth(
        &self,
        token: &str,
        origin: Option<&HeaderValue>,
    ) -> Result<RemotePrincipal, RemoteAuthError> {
        self.verify_origin(origin)?;
        self.verify_bearer(token)
    }

    pub fn authorize(
        &self,
        principal: &RemotePrincipal,
        capability: RemoteCapability,
    ) -> Result<(), RemoteAuthError> {
        let credential = self.credential.read();
        if credential.digest.is_none()
            || principal.generation != credential.generation
            || principal.authority_source != credential.source
            || principal.authority_source == RemoteAuthoritySource::Unconfigured
        {
            log::warn!(
                target: "se_manager::web::auth",
                "operation=capability_authorize generation={} capability={} stable_code={}",
                principal.generation,
                capability.as_str(),
                RemoteAuthError::Forbidden.code()
            );
            return Err(RemoteAuthError::Forbidden);
        }
        Ok(())
    }

    pub fn verify_origin(&self, origin: Option<&HeaderValue>) -> Result<(), RemoteAuthError> {
        let raw = origin
            .and_then(|value| value.to_str().ok())
            .ok_or(RemoteAuthError::InvalidOrigin)?;
        let parsed = Url::parse(raw).map_err(|_| RemoteAuthError::InvalidOrigin)?;
        let normalized = normalize_origin(&parsed)?;
        if self.allowed_origins.read().contains(&normalized) {
            Ok(())
        } else {
            Err(RemoteAuthError::InvalidOrigin)
        }
    }

    pub fn verify_bearer_for_peer(
        &self,
        token: &str,
        peer: IpAddr,
    ) -> Result<RemotePrincipal, RemoteAuthError> {
        self.verify_bearer_for_peer_at(token, peer, Instant::now())
    }

    fn verify_bearer_for_peer_at(
        &self,
        token: &str,
        peer: IpAddr,
        now: Instant,
    ) -> Result<RemotePrincipal, RemoteAuthError> {
        // Always verify the current credential before consulting failure state.
        // A forged burst through one shared loopback proxy therefore cannot
        // lock out a caller that presents the correct generation bearer.
        match self.verify_bearer(token) {
            Ok(principal) => Ok(principal),
            Err(error) => {
                let generation = {
                    let credential = self.credential.read();
                    if credential.generation == 0
                        || credential.source == RemoteAuthoritySource::Unconfigured
                    {
                        return Err(error);
                    }
                    credential.generation
                };
                let reported = self.failures.lock().record_failure(
                    FailureKey {
                        identity: failure_identity(peer),
                        generation,
                    },
                    now,
                );
                log::warn!(
                    target: "se_manager::web::auth",
                    "operation=bearer_verify generation={} auth_class=bearer stable_code={}",
                    generation,
                    reported.code()
                );
                Err(reported)
            }
        }
    }

    /// Number of retained unauthenticated failure states after TTL cleanup.
    #[must_use]
    pub fn failure_state_count(&self) -> usize {
        self.failures.lock().len_at(Instant::now())
    }

    #[cfg(test)]
    fn failure_state_count_at(&self, now: Instant) -> usize {
        self.failures.lock().len_at(now)
    }
}

fn generate_token() -> Result<String, RemoteAuthError> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    getrandom::getrandom(&mut bytes).map_err(|_| RemoteAuthError::Provisioning)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn validate_token(token: &str) -> Result<(), RemoteAuthError> {
    if token.is_empty() || token.len() > MAX_TOKEN_BYTES || token.trim() != token {
        return Err(RemoteAuthError::InvalidCredential);
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn normalize_origin(origin: &Url) -> Result<String, RemoteAuthError> {
    if !matches!(origin.scheme(), "http" | "https")
        || origin.host_str().is_none()
        || origin.username() != ""
        || origin.password().is_some()
    {
        return Err(RemoteAuthError::InvalidOrigin);
    }
    let mut normalized = format!("{}://{}", origin.scheme(), origin.host_str().unwrap());
    if let Some(port) = origin.port() {
        normalized.push(':');
        normalized.push_str(&port.to_string());
    }
    Ok(normalized)
}

fn read_token_from_validated_handle(file: &mut File) -> Result<String, RemoteAuthError> {
    let mut bytes = Vec::with_capacity(MAX_TOKEN_BYTES + 1);
    file.take((MAX_TOKEN_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| RemoteAuthError::Provisioning)?;
    if bytes.len() > MAX_TOKEN_BYTES {
        return Err(RemoteAuthError::Provisioning);
    }
    let token = String::from_utf8(bytes).map_err(|_| RemoteAuthError::Provisioning)?;
    let token = token.trim_end_matches(['\r', '\n']).to_string();
    validate_token(&token)?;
    Ok(token)
}

#[cfg(unix)]
fn open_validated_token_file(path: &Path) -> Result<File, RemoteAuthError> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| RemoteAuthError::Provisioning)?;
    let metadata = file.metadata().map_err(|_| RemoteAuthError::Provisioning)?;
    // SAFETY: `geteuid` has no preconditions and only reads the process's
    // effective user id. All checks are handle-derived (`fstat` semantics).
    let effective_uid = unsafe { libc::geteuid() };
    validate_unix_token_metadata(
        metadata.file_type().is_file(),
        metadata.uid(),
        metadata.mode(),
        metadata.len(),
        effective_uid,
    )?;
    Ok(file)
}

#[cfg(unix)]
fn validate_unix_token_metadata(
    regular: bool,
    owner_uid: u32,
    mode: u32,
    length: u64,
    effective_uid: u32,
) -> Result<(), RemoteAuthError> {
    if !regular
        || owner_uid != effective_uid
        || mode & 0o077 != 0
        || length > MAX_TOKEN_BYTES as u64
    {
        return Err(RemoteAuthError::Provisioning);
    }
    Ok(())
}

#[cfg(windows)]
fn open_validated_token_file(path: &Path) -> Result<File, RemoteAuthError> {
    windows_token_file::open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_validated_token_file(_path: &Path) -> Result<File, RemoteAuthError> {
    Err(RemoteAuthError::Provisioning)
}

#[cfg(windows)]
mod windows_token_file {
    use super::{File, Path, RemoteAuthError, MAX_TOKEN_BYTES};
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::ptr::{null_mut, NonNull};

    use windows_sys::Win32::Foundation::{
        CloseHandle, LocalFree, ERROR_SUCCESS, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AclSizeInformation, CreateWellKnownSid, EqualSid, GetAce, GetAclInformation,
        GetTokenInformation, IsValidSid, TokenUser, WinLocalSystemSid, ACCESS_ALLOWED_ACE,
        ACE_HEADER, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileAttributeTagInfo, GetFileInformationByHandleEx, GetFileSizeEx,
        GetFileType, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_TYPE_DISK,
        OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const ACCESS_ALLOWED_OBJECT_ACE_TYPE: u8 = 5;
    const ACCESS_ALLOWED_CALLBACK_ACE_TYPE: u8 = 9;
    const ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE: u8 = 11;
    const SECURITY_MAX_SID_SIZE: usize = 68;

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: this guard owns one valid Win32 handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

    impl Drop for SecurityDescriptor {
        fn drop(&mut self) {
            // SAFETY: GetSecurityInfo allocates this descriptor with LocalAlloc.
            unsafe {
                let _ = LocalFree(self.0.cast());
            }
        }
    }

    pub(super) fn open(path: &Path) -> Result<File, RemoteAuthError> {
        let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        wide.push(0);
        // SAFETY: the UTF-16 path is NUL-terminated; returned ownership is
        // transferred exactly once into `File` below.
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(RemoteAuthError::Provisioning);
        }
        let handle = Handle(raw);
        validate_file_identity(handle.0)?;
        validate_security(handle.0)?;
        // SAFETY: `handle` uniquely owns `raw`; forget the guard after moving
        // ownership into `File` so the handle is closed exactly once.
        let file = unsafe { File::from_raw_handle(handle.0) };
        std::mem::forget(handle);
        Ok(file)
    }

    fn validate_file_identity(handle: HANDLE) -> Result<(), RemoteAuthError> {
        // SAFETY: `handle` is valid and all output buffers have exact sizes.
        unsafe {
            if GetFileType(handle) != FILE_TYPE_DISK {
                return Err(RemoteAuthError::Provisioning);
            }
            let mut tag: FILE_ATTRIBUTE_TAG_INFO = zeroed();
            if GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            ) == 0
            {
                return Err(RemoteAuthError::Provisioning);
            }
            if tag.FileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
                return Err(RemoteAuthError::Provisioning);
            }
            let mut length = 0_i64;
            if GetFileSizeEx(handle, &mut length) == 0
                || length < 0
                || length as u64 > MAX_TOKEN_BYTES as u64
            {
                return Err(RemoteAuthError::Provisioning);
            }
        }
        Ok(())
    }

    fn validate_security(handle: HANDLE) -> Result<(), RemoteAuthError> {
        let mut owner: PSID = null_mut();
        let mut dacl = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: all pointers are valid out-parameters; descriptor ownership
        // is released by `SecurityDescriptor` on every return path.
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS || descriptor.is_null() {
            return Err(RemoteAuthError::Provisioning);
        }
        let _descriptor = SecurityDescriptor(descriptor);
        let current_user = current_user_sid()?;
        // SAFETY: owner and current-user SIDs are backed by live descriptors.
        if owner.is_null()
            || unsafe { IsValidSid(owner) } == 0
            || unsafe { EqualSid(owner, current_user.as_ptr()) } == 0
        {
            return Err(RemoteAuthError::Provisioning);
        }
        validate_dacl(dacl, current_user.as_ptr())
    }

    struct SidBuffer {
        words: Vec<usize>,
    }

    impl SidBuffer {
        fn with_byte_capacity(bytes: usize) -> Self {
            let words = bytes.div_ceil(size_of::<usize>());
            Self {
                words: vec![0; words.max(1)],
            }
        }

        fn as_ptr(&self) -> PSID {
            self.words.as_ptr().cast_mut().cast()
        }

        fn as_mut_ptr(&mut self) -> *mut c_void {
            self.words.as_mut_ptr().cast()
        }

        fn byte_capacity(&self) -> u32 {
            (self.words.len() * size_of::<usize>()) as u32
        }
    }

    fn current_user_sid() -> Result<SidBuffer, RemoteAuthError> {
        let mut token: HANDLE = null_mut();
        // SAFETY: current process pseudo-handle is valid; `token` is an out-param.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(RemoteAuthError::Provisioning);
        }
        let token = Handle(token);
        let mut required = 0_u32;
        // The first call intentionally obtains the required size.
        unsafe {
            GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required);
        }
        if required < size_of::<TOKEN_USER>() as u32 {
            return Err(RemoteAuthError::Provisioning);
        }
        let mut raw = SidBuffer::with_byte_capacity(required as usize);
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                raw.as_mut_ptr(),
                raw.byte_capacity(),
                &mut required,
            )
        } == 0
        {
            return Err(RemoteAuthError::Provisioning);
        }
        // Copy the SID out of the TOKEN_USER buffer so its address remains
        // stable independently of the token-information layout.
        let token_user = unsafe { &*(raw.words.as_ptr().cast::<TOKEN_USER>()) };
        if token_user.User.Sid.is_null() || unsafe { IsValidSid(token_user.User.Sid) } == 0 {
            return Err(RemoteAuthError::Provisioning);
        }
        copy_sid(token_user.User.Sid)
    }

    fn copy_sid(sid: PSID) -> Result<SidBuffer, RemoteAuthError> {
        use windows_sys::Win32::Security::{CopySid, GetLengthSid};
        // SAFETY: caller supplies a validated SID.
        let length = unsafe { GetLengthSid(sid) };
        if length == 0 {
            return Err(RemoteAuthError::Provisioning);
        }
        let mut copy = SidBuffer::with_byte_capacity(length as usize);
        if unsafe { CopySid(copy.byte_capacity(), copy.as_ptr(), sid) } == 0 {
            return Err(RemoteAuthError::Provisioning);
        }
        Ok(copy)
    }

    fn well_known_sid(kind: i32) -> Result<SidBuffer, RemoteAuthError> {
        let mut sid = SidBuffer::with_byte_capacity(SECURITY_MAX_SID_SIZE);
        let mut length = sid.byte_capacity();
        // SAFETY: the aligned buffer is writable for `length` bytes.
        if unsafe { CreateWellKnownSid(kind, null_mut(), sid.as_ptr(), &mut length) } == 0 {
            return Err(RemoteAuthError::Provisioning);
        }
        Ok(sid)
    }

    fn validate_dacl(
        dacl: *mut windows_sys::Win32::Security::ACL,
        current_user: PSID,
    ) -> Result<(), RemoteAuthError> {
        let Some(dacl) = NonNull::new(dacl) else {
            return Err(RemoteAuthError::Provisioning);
        };
        if current_user.is_null() || unsafe { IsValidSid(current_user) } == 0 {
            return Err(RemoteAuthError::Provisioning);
        }
        let mut info: ACL_SIZE_INFORMATION = unsafe { zeroed() };
        // SAFETY: DACL is descriptor-owned and `info` has the requested layout.
        if unsafe {
            GetAclInformation(
                dacl.as_ptr(),
                (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            return Err(RemoteAuthError::Provisioning);
        }
        let local_system = well_known_sid(WinLocalSystemSid)?;
        for index in 0..info.AceCount {
            let mut ace: *mut c_void = null_mut();
            if unsafe { GetAce(dacl.as_ptr(), index, &mut ace) } == 0 || ace.is_null() {
                return Err(RemoteAuthError::Provisioning);
            }
            let header = unsafe { &*(ace.cast::<ACE_HEADER>()) };
            if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 {
                continue;
            }
            if matches!(
                header.AceType,
                ACCESS_ALLOWED_OBJECT_ACE_TYPE
                    | ACCESS_ALLOWED_CALLBACK_ACE_TYPE
                    | ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE
            ) {
                // Object/callback allow ACE layouts are variable. Fail closed
                // rather than guess where their SID begins.
                return Err(RemoteAuthError::Provisioning);
            }
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
                continue;
            }
            if usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>() {
                return Err(RemoteAuthError::Provisioning);
            }
            let allowed = unsafe { &*(ace.cast::<ACCESS_ALLOWED_ACE>()) };
            let sid = (&allowed.SidStart as *const u32).cast_mut().cast();
            if unsafe { IsValidSid(sid) } == 0 {
                return Err(RemoteAuthError::Provisioning);
            }
            if allowed.Mask == 0 {
                continue;
            }
            let permitted = unsafe { EqualSid(sid, current_user) } != 0
                || unsafe { EqualSid(sid, local_system.as_ptr()) } != 0;
            if !permitted {
                return Err(RemoteAuthError::Provisioning);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn validate_descriptor_for_tests(
        owner: PSID,
        dacl: *mut windows_sys::Win32::Security::ACL,
        current_user: PSID,
    ) -> Result<(), RemoteAuthError> {
        if owner.is_null()
            || current_user.is_null()
            || unsafe { IsValidSid(owner) } == 0
            || unsafe { IsValidSid(current_user) } == 0
            || unsafe { EqualSid(owner, current_user) } == 0
        {
            return Err(RemoteAuthError::Provisioning);
        }
        validate_dacl(dacl, current_user)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthFailureBody {
    success: bool,
    error: &'static str,
    code: &'static str,
}

pub fn auth_error_response(error: RemoteAuthError) -> Response {
    (
        error.status(),
        Json(AuthFailureBody {
            success: false,
            error: error.safe_message(),
            code: error.code(),
        }),
    )
        .into_response()
}

pub async fn capability_middleware(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let route_class = request
        .extensions()
        .get::<RemoteRouteClass>()
        .copied()
        .or_else(|| RemoteRouteClass::from_path(request.uri().path()));
    let Some(route_class) = route_class else {
        return next.run(request).await;
    };
    let method = request.method().clone();
    let Some(capability) = route_class.capability(&method) else {
        return next.run(request).await;
    };
    let provenance = request
        .extensions()
        .get::<IngressProvenance>()
        .copied()
        .unwrap_or_else(|| authority.ingress_provenance());
    let started = Instant::now();

    // ACP and terminal WebSocket bearer authentication occur in the first
    // protocol frame; this HTTP boundary records only identifier-free route
    // metadata. Optional handshake Authorization is still honored by the
    // upgrade handlers when a client can send it.
    if !route_class.requires_http_bearer() {
        if route_class == RemoteRouteClass::TerminalWebSocket {
            if let Some(response) =
                reject_invalid_optional_terminal_bearer(&authority, &request, provenance, started)
            {
                return response;
            }
        }
        let response = next.run(request).await;
        let stable_code = if response.status().is_success()
            || response.status() == StatusCode::SWITCHING_PROTOCOLS
        {
            "OK"
        } else {
            "APPLICATION_ERROR"
        };
        log_boundary_outcome(
            &method,
            route_class,
            capability,
            provenance,
            stable_code,
            response.status(),
            started.elapsed(),
        );
        return response;
    }

    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::from([0, 0, 0, 0]), |value| value.0.ip());
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(token) = token else {
        let error = authority
            .verify_bearer_for_peer("", peer)
            .err()
            .unwrap_or(RemoteAuthError::InvalidCredential);
        log_boundary_outcome(
            &method,
            route_class,
            capability,
            provenance,
            error.code(),
            error.status(),
            started.elapsed(),
        );
        return auth_error_response(error);
    };
    let principal = match authority.verify_bearer_for_peer(token, peer) {
        Ok(principal) => principal,
        Err(error) => {
            log_boundary_outcome(
                &method,
                route_class,
                capability,
                provenance,
                error.code(),
                error.status(),
                started.elapsed(),
            );
            return auth_error_response(error);
        }
    };
    if let Err(error) = authority.authorize(&principal, capability) {
        log_boundary_outcome(
            &method,
            route_class,
            capability,
            provenance,
            error.code(),
            error.status(),
            started.elapsed(),
        );
        return auth_error_response(error);
    }
    request.extensions_mut().insert(principal);
    let response = next.run(request).await;
    let stable_code = if response.status().is_success() {
        "OK"
    } else {
        "APPLICATION_ERROR"
    };
    log_boundary_outcome(
        &method,
        route_class,
        capability,
        provenance,
        stable_code,
        response.status(),
        started.elapsed(),
    );
    response
}

fn reject_invalid_optional_terminal_bearer(
    authority: &RemoteAccessAuthority,
    request: &Request<Body>,
    provenance: IngressProvenance,
    started: Instant,
) -> Option<Response> {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())?;
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::from([0, 0, 0, 0]), |value| value.0.ip());
    match authority.verify_bearer_for_peer(token, peer) {
        Ok(principal) => {
            if let Err(error) = authority.authorize(&principal, RemoteCapability::Mutate) {
                log_boundary_outcome(
                    request.method(),
                    RemoteRouteClass::TerminalWebSocket,
                    RemoteCapability::Mutate,
                    provenance,
                    error.code(),
                    error.status(),
                    started.elapsed(),
                );
                return Some(auth_error_response(error));
            }
            None
        }
        Err(error) => {
            log_boundary_outcome(
                request.method(),
                RemoteRouteClass::TerminalWebSocket,
                RemoteCapability::Mutate,
                provenance,
                error.code(),
                error.status(),
                started.elapsed(),
            );
            Some(auth_error_response(error))
        }
    }
}

fn log_boundary_outcome(
    method: &Method,
    route_class: RemoteRouteClass,
    capability: RemoteCapability,
    provenance: IngressProvenance,
    stable_code: &str,
    status: StatusCode,
    duration: Duration,
) {
    if status.is_client_error() || status.is_server_error() {
        log::warn!(
            target: "se_manager::web::auth",
            "operation=remote_boundary method={} route_class={} capability={} provenance={} stable_code={} http_status={} duration_ms={}",
            method.as_str(),
            route_class.as_str(),
            capability.as_str(),
            provenance.as_str(),
            stable_code,
            status.as_u16(),
            duration.as_millis()
        );
    } else {
        log::info!(
            target: "se_manager::web::auth",
            "operation=remote_boundary method={} route_class={} capability={} provenance={} stable_code={} http_status={} duration_ms={}",
            method.as_str(),
            route_class.as_str(),
            capability.as_str(),
            provenance.as_str(),
            stable_code,
            status.as_u16(),
            duration.as_millis()
        );
    }
}

/// Stable application-code to HTTP-status mapping shared by Conversation
/// adapters. Bodies retain their existing camelCase `IpcBody<T>` envelope.
#[must_use]
pub fn status_for_code(code: &str) -> StatusCode {
    match code {
        "VALIDATION_ERROR" | "CONVERSATION_INVALID_ID" => StatusCode::BAD_REQUEST,
        "UNAUTHORIZED" => StatusCode::UNAUTHORIZED,
        "FORBIDDEN" => StatusCode::FORBIDDEN,
        "CONVERSATION_NOT_FOUND" | "RECOVERY_NOT_FOUND" => StatusCode::NOT_FOUND,
        "CONVERSATION_CONFLICT"
        | "CONVERSATION_LIVE_RESOURCES"
        | "LEGACY_ID_AMBIGUOUS"
        | "MIGRATION_IDEMPOTENCY_CONFLICT" => StatusCode::CONFLICT,
        "CONVERSATION_RECOVERY_REQUIRED"
        | "ACP_COMPENSATION_FAILED"
        | "LEGACY_COMPATIBILITY_READ_ONLY" => StatusCode::UNPROCESSABLE_ENTITY,
        "CONVERSATION_SERVICE_UNAVAILABLE" | "SESSION_WORKSPACE_UNAVAILABLE" => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
pub mod test_tracing {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Mutex as StdMutex, Once};

    pub const WIN_LOCAL_SYSTEM_SID: &str = "S-1-5-18";
    pub const WIN_NETWORK_SERVICE_SID: &str = "S-1-5-20";

    #[derive(Clone)]
    struct CapturedRecord {
        scope_id: u64,
        target: String,
        message: String,
    }

    struct CaptureLogger {
        next_id: AtomicU64,
        active_id: AtomicU64,
        records: StdMutex<Vec<CapturedRecord>>,
        forwarded: StdMutex<VecDeque<CapturedRecord>>,
    }

    impl log::Log for CaptureLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            let captured = CapturedRecord {
                scope_id: self.active_id.load(Ordering::Acquire),
                target: record.target().to_string(),
                message: record.args().to_string(),
            };
            if captured.scope_id != 0 {
                self.records.lock().unwrap().push(captured);
                return;
            }
            let mut forwarded = self.forwarded.lock().unwrap();
            if forwarded.len() == 64 {
                forwarded.pop_front();
            }
            forwarded.push_back(captured);
        }

        fn flush(&self) {}
    }

    static LOGGER: CaptureLogger = CaptureLogger {
        next_id: AtomicU64::new(1),
        active_id: AtomicU64::new(0),
        records: StdMutex::new(Vec::new()),
        forwarded: StdMutex::new(VecDeque::new()),
    };
    static INSTALL: Once = Once::new();
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    static HARNESS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    pub struct Guard {
        id: u64,
        _guard: tokio::sync::MutexGuard<'static, ()>,
    }

    impl Guard {
        #[must_use]
        pub fn id(&self) -> u64 {
            self.id
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            LOGGER.active_id.store(0, Ordering::Release);
            LOGGER
                .records
                .lock()
                .unwrap()
                .retain(|record| record.scope_id != self.id);
        }
    }

    pub fn install_forwarding_logger() {
        INSTALL.call_once(|| {
            if log::set_logger(&LOGGER).is_ok() {
                log::set_max_level(log::LevelFilter::Trace);
                INSTALLED.store(true, Ordering::Release);
            }
        });
    }

    pub async fn lock() -> Guard {
        lock_scoped("default").await
    }

    pub async fn lock_scoped(_scope: &'static str) -> Guard {
        let guard = HARNESS.lock().await;
        install_forwarding_logger();
        assert!(
            INSTALLED.load(Ordering::Acquire),
            "shared test logger must install before boundary capture"
        );
        let id = LOGGER.next_id.fetch_add(1, Ordering::AcqRel);
        LOGGER
            .records
            .lock()
            .unwrap()
            .retain(|record| record.scope_id == id);
        LOGGER.active_id.store(id, Ordering::Release);
        Guard { id, _guard: guard }
    }

    pub fn messages(target: &str) -> Vec<String> {
        let active = LOGGER.active_id.load(Ordering::Acquire);
        messages_for(active, target)
    }

    pub fn messages_for(scope_id: u64, target: &str) -> Vec<String> {
        LOGGER
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.target == target && record.scope_id == scope_id)
            .map(|record| record.message.clone())
            .collect()
    }

    pub fn emit_unscoped_for_tests(target: &str, message: &str) {
        let previous = LOGGER.active_id.swap(0, Ordering::AcqRel);
        log::info!(target: target, "{message}");
        LOGGER.active_id.store(previous, Ordering::Release);
    }

    pub fn forwarded_messages(target: &str) -> Vec<String> {
        LOGGER
            .forwarded
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.target == target && record.scope_id == 0)
            .map(|record| record.message.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use std::net::Ipv6Addr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    const TOKEN: &str = "test-remote-access-token";

    fn authority() -> RemoteAccessAuthority {
        let authority = RemoteAccessAuthority::for_tests(TOKEN);
        authority
            .set_public_origin(Url::parse("https://example.test/path").unwrap())
            .unwrap();
        authority
    }

    #[test]
    fn credential_verification_rejects_empty_wrong_and_oversized() {
        let authority = authority();
        assert!(authority.verify_bearer(TOKEN).is_ok());
        assert_eq!(
            authority.verify_bearer("").unwrap_err(),
            RemoteAuthError::InvalidCredential
        );
        assert_eq!(
            authority.verify_bearer("wrong").unwrap_err(),
            RemoteAuthError::InvalidCredential
        );
        assert_eq!(
            authority
                .verify_bearer(&"x".repeat(MAX_TOKEN_BYTES + 1))
                .unwrap_err(),
            RemoteAuthError::InvalidCredential
        );
    }

    #[test]
    fn digest_only_constant_time_comparison_is_pinned() {
        let source = include_str!("auth.rs");
        assert!(source.contains("Sha256::digest"));
        assert!(source.contains("ct_eq"));
        assert!(!source.contains(&["credential", ": String"].concat()));
        assert!(!source.contains(&["token", " =="].concat()));
    }

    #[test]
    fn websocket_origin_is_required_and_normalized() {
        let authority = authority();
        let allowed = HeaderValue::from_static("https://example.test");
        let wrong = HeaderValue::from_static("https://evil.test");
        assert!(authority.verify_ws_auth(TOKEN, Some(&allowed)).is_ok());
        assert_eq!(
            authority.verify_ws_auth(TOKEN, None).unwrap_err(),
            RemoteAuthError::InvalidOrigin
        );
        assert_eq!(
            authority.verify_ws_auth(TOKEN, Some(&wrong)).unwrap_err(),
            RemoteAuthError::InvalidOrigin
        );
    }

    #[test]
    fn lan_peer_failures_do_not_share_the_proxied_loopback_bucket() {
        let authority = authority();
        let loopback = IpAddr::from([127, 0, 0, 1]);
        let lan_a = IpAddr::from([10, 0, 0, 5]);
        let lan_b = IpAddr::from([10, 0, 0, 6]);
        for _ in 0..FAILURE_LIMIT {
            assert_eq!(
                authority
                    .verify_bearer_for_peer("wrong", loopback)
                    .unwrap_err(),
                RemoteAuthError::InvalidCredential
            );
        }
        assert_eq!(
            authority
                .verify_bearer_for_peer("wrong", loopback)
                .unwrap_err(),
            RemoteAuthError::RateLimited
        );
        assert_eq!(
            authority
                .verify_bearer_for_peer("wrong", lan_a)
                .unwrap_err(),
            RemoteAuthError::InvalidCredential
        );
        assert!(authority.verify_bearer_for_peer(TOKEN, lan_a).is_ok());
        assert!(authority.verify_bearer_for_peer(TOKEN, lan_b).is_ok());
    }

    #[test]
    fn forged_proxy_failures_rate_limit_invalid_tokens_but_never_the_valid_bearer() {
        let authority = authority();
        let peer = IpAddr::from([127, 0, 0, 1]);
        for _ in 0..FAILURE_LIMIT {
            assert_eq!(
                authority.verify_bearer_for_peer("wrong", peer).unwrap_err(),
                RemoteAuthError::InvalidCredential
            );
        }
        assert_eq!(
            authority.verify_bearer_for_peer("wrong", peer).unwrap_err(),
            RemoteAuthError::RateLimited
        );
        assert!(authority.verify_bearer_for_peer(TOKEN, peer).is_ok());
        assert_eq!(
            authority.verify_bearer_for_peer("wrong", peer).unwrap_err(),
            RemoteAuthError::RateLimited
        );
    }

    #[test]
    fn desktop_generation_rotation_invalidates_stale_bearers() {
        let authority = authority();
        let first = authority.rotate_desktop_credential().unwrap();
        let first_bearer = first.bearer().to_string();
        assert!(authority.verify_bearer(&first_bearer).is_ok());
        authority.invalidate_generation(first.generation());
        assert_eq!(
            authority.verify_bearer(&first_bearer).unwrap_err(),
            RemoteAuthError::InvalidCredential
        );

        let second = authority.rotate_desktop_credential().unwrap();
        assert!(second.generation() > first.generation());
        assert_ne!(second.bearer(), first_bearer);
        assert_eq!(
            authority.verify_bearer(&first_bearer).unwrap_err(),
            RemoteAuthError::InvalidCredential
        );
        assert!(authority.verify_bearer(second.bearer()).is_ok());
    }

    #[test]
    fn failure_state_is_ttl_lru_bounded_under_high_cardinality_attack() {
        let authority = authority();
        let now = Instant::now();
        for index in 0_u128..10_000 {
            let peer = IpAddr::V6(Ipv6Addr::from(index + 1));
            assert_eq!(
                authority
                    .verify_bearer_for_peer_at("wrong", peer, now)
                    .unwrap_err(),
                RemoteAuthError::InvalidCredential
            );
        }
        assert!(authority.failure_state_count_at(now) <= MAX_AUTH_FAILURE_STATES);
        assert_eq!(
            authority.failure_state_count_at(now + FAILURE_STATE_TTL + Duration::from_secs(1)),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn token_file_same_handle_rejects_swaps_nonregular_foreign_owner_and_oversize() {
        use std::io::Write;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("token");
        std::fs::write(&original, TOKEN).unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();

        let mut validated = open_validated_token_file(&original).unwrap();
        let retained = dir.path().join("validated-token");
        std::fs::rename(&original, &retained).unwrap();
        std::fs::write(&original, "attacker-replacement-token").unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            read_token_from_validated_handle(&mut validated).unwrap(),
            TOKEN
        );

        let metadata = std::fs::metadata(&retained).unwrap();
        assert_eq!(
            validate_unix_token_metadata(
                true,
                metadata.uid().saturating_add(1),
                metadata.mode(),
                metadata.len(),
                metadata.uid(),
            ),
            Err(RemoteAuthError::Provisioning)
        );
        assert_eq!(
            open_validated_token_file(dir.path()).unwrap_err(),
            RemoteAuthError::Provisioning
        );

        let oversized = dir.path().join("oversized");
        let mut oversized_file = std::fs::File::create(&oversized).unwrap();
        oversized_file
            .write_all(&vec![b'x'; MAX_TOKEN_BYTES + 1])
            .unwrap();
        drop(oversized_file);
        std::fs::set_permissions(&oversized, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            RemoteAccessAuthority::from_token_file(&oversized).unwrap_err(),
            RemoteAuthError::Provisioning
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_token_descriptor_rejects_foreign_owner_null_dacl_and_broad_allow_ace() {
        use std::ffi::c_void;
        use std::mem::size_of;
        use std::ptr::null_mut;
        use windows_sys::Win32::Foundation::GENERIC_READ;
        use windows_sys::Win32::Security::{
            AddAccessAllowedAce, CreateWellKnownSid, InitializeAcl, WinAuthenticatedUserSid,
            WinBuiltinAdministratorsSid, WinWorldSid, ACL, ACL_REVISION,
        };

        fn sid(kind: i32) -> Vec<usize> {
            let mut storage = vec![0_usize; 16];
            let mut bytes = (storage.len() * size_of::<usize>()) as u32;
            assert_ne!(
                unsafe {
                    CreateWellKnownSid(
                        kind,
                        null_mut(),
                        storage.as_mut_ptr().cast::<c_void>(),
                        &mut bytes,
                    )
                },
                0
            );
            storage
        }

        let owner = sid(WinBuiltinAdministratorsSid);
        let foreign = sid(WinAuthenticatedUserSid);
        let owner_ptr = owner.as_ptr().cast_mut().cast();
        let foreign_ptr = foreign.as_ptr().cast_mut().cast();
        assert_eq!(
            windows_token_file::validate_descriptor_for_tests(foreign_ptr, null_mut(), owner_ptr,),
            Err(RemoteAuthError::Provisioning)
        );
        assert_eq!(
            windows_token_file::validate_descriptor_for_tests(owner_ptr, null_mut(), owner_ptr),
            Err(RemoteAuthError::Provisioning)
        );

        let world = sid(WinWorldSid);
        let mut acl_storage = vec![0_usize; 128];
        let acl = acl_storage.as_mut_ptr().cast::<ACL>();
        assert_ne!(
            unsafe {
                InitializeAcl(
                    acl,
                    (acl_storage.len() * size_of::<usize>()) as u32,
                    ACL_REVISION,
                )
            },
            0
        );
        assert_ne!(
            unsafe {
                AddAccessAllowedAce(
                    acl,
                    ACL_REVISION,
                    GENERIC_READ,
                    world.as_ptr().cast_mut().cast(),
                )
            },
            0
        );
        assert_eq!(
            windows_token_file::validate_descriptor_for_tests(owner_ptr, acl, owner_ptr),
            Err(RemoteAuthError::Provisioning)
        );

        let source = include_str!("auth.rs");
        assert!(source.contains("FILE_FLAG_OPEN_REPARSE_POINT"));
        assert!(source.contains("FILE_ATTRIBUTE_REPARSE_POINT"));
    }

    #[cfg(unix)]
    #[test]
    fn token_file_rejects_group_or_world_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, TOKEN).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            RemoteAccessAuthority::from_token_file(&path).unwrap_err(),
            RemoteAuthError::Provisioning
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(RemoteAccessAuthority::from_token_file(&path).is_ok());

        let link = dir.path().join("token-link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(
            RemoteAccessAuthority::from_token_file(&link).unwrap_err(),
            RemoteAuthError::Provisioning
        );
    }

    #[test]
    fn generation_retirement_revokes_digest_without_touching_keyring() {
        let authority =
            RemoteAccessAuthority::from_token(TOKEN, RemoteAuthoritySource::DesktopKeyring);
        authority
            .set_public_origin(Url::parse("https://retire.example.test").unwrap())
            .unwrap();
        let receipt = authority.retire_generation(1);
        assert!(receipt.credential_invalidated);
        assert!(receipt.origins_cleared);
        assert!(receipt.failure_state_cleared);
        assert!(receipt.keyring_deleted);
        assert!(receipt.stable_codes.is_empty());
        assert_eq!(
            authority.verify_bearer(TOKEN).unwrap_err(),
            RemoteAuthError::InvalidCredential
        );
    }

    #[test]
    fn adopt_reuses_settings_bearer_and_issue_mints_when_absent() {
        let authority = RemoteAccessAuthority::desktop_memory();
        let minted = generate_token().unwrap();
        let (first, issued) = authority
            .adopt_or_issue_desktop_credential(Some(&minted))
            .unwrap();
        assert!(!issued);
        assert_eq!(first.bearer(), minted);
        assert!(authority.verify_bearer(&minted).is_ok());

        let empty = RemoteAccessAuthority::desktop_memory();
        let (second, issued) = empty.adopt_or_issue_desktop_credential(None).unwrap();
        assert!(issued);
        assert_ne!(second.bearer(), minted);
        assert!(empty.verify_bearer(second.bearer()).is_ok());
    }

    fn protected_test_router(authority: Arc<RemoteAccessAuthority>) -> axum::Router {
        axum::Router::new()
            .route(
                "/conversations",
                get(|| async { "sensitive-workspace-path" }),
            )
            .layer(axum::middleware::from_fn(capability_middleware))
            .layer(Extension(RemoteRouteClass::Conversation))
            .layer(Extension(IngressProvenance::LocalOperator))
            .layer(Extension(authority))
    }

    fn protected_request(authorization: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .uri("/conversations")
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 3000))));
        if let Some(value) = authorization {
            builder = builder.header(header::AUTHORIZATION, value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn protected_http_rejects_missing_wrong_and_oversized_credentials_without_body_leak() {
        let _boundary_log_test_guard = test_tracing::lock().await;
        let oversized = format!("Bearer {}", "x".repeat(MAX_TOKEN_BYTES + 1));
        for authorization in [None, Some("Bearer wrong"), Some(oversized.as_str())] {
            let app = protected_test_router(Arc::new(authority()));
            let response = app.oneshot(protected_request(authorization)).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let text = String::from_utf8_lossy(&body);
            assert!(text.contains("UNAUTHORIZED"));
            assert!(!text.contains("sensitive-workspace-path"));
        }
    }

    #[tokio::test]
    async fn loopback_proxy_without_credential_cannot_reach_lifecycle_or_workspace_mutations() {
        let _boundary_log_test_guard = test_tracing::lock().await;
        let reached = Arc::new(AtomicUsize::new(0));
        let handler_reached = Arc::clone(&reached);
        let authority = Arc::new(authority());
        let app = axum::Router::new()
            .route(
                "/conversations/{conversationId}/lifecycle/detach",
                post(move || {
                    let handler_reached = Arc::clone(&handler_reached);
                    async move {
                        handler_reached.fetch_add(1, Ordering::SeqCst);
                        StatusCode::NO_CONTENT
                    }
                }),
            )
            .route(
                "/conversations/{conversationId}/workspace",
                post({
                    let reached = Arc::clone(&reached);
                    move || {
                        let reached = Arc::clone(&reached);
                        async move {
                            reached.fetch_add(1, Ordering::SeqCst);
                            StatusCode::NO_CONTENT
                        }
                    }
                }),
            )
            .layer(axum::middleware::from_fn(capability_middleware))
            .layer(Extension(RemoteRouteClass::Conversation))
            .layer(Extension(authority));

        for uri in [
            "/conversations/c-1/lifecycle/detach",
            "/conversations/c-1/workspace",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(uri)
                        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 43123))))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
        }
        assert_eq!(reached.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn protected_http_accepts_bearer_and_rate_limits_sixth_failure() {
        let _boundary_log_test_guard = test_tracing::lock().await;
        let authority = Arc::new(authority());
        let app = protected_test_router(Arc::clone(&authority));
        let accepted = app
            .clone()
            .oneshot(protected_request(Some(&format!("Bearer {TOKEN}"))))
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);

        for attempt in 1..=FAILURE_LIMIT + 1 {
            let response = app
                .clone()
                .oneshot(protected_request(Some("Bearer wrong")))
                .await
                .unwrap();
            let expected = if attempt > FAILURE_LIMIT {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::UNAUTHORIZED
            };
            assert_eq!(response.status(), expected, "attempt {attempt}");
        }
    }

    #[tokio::test]
    async fn captured_boundary_log_uses_static_class_without_path_identifier_or_credential() {
        let _boundary_log_test_guard = test_tracing::lock().await;
        let authority = Arc::new(authority());
        let app = axum::Router::new()
            .route(
                "/conversations/{conversationId}/lifecycle/detach",
                post(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(axum::middleware::from_fn(capability_middleware))
            .layer(Extension(RemoteRouteClass::Conversation))
            .layer(Extension(IngressProvenance::PublicTunnel))
            .layer(Extension(authority));
        let supplied_path = "/conversations/supplied-conversation-id/lifecycle/detach";
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(supplied_path)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 43123))))
                    .body(Body::from("supplied-sensitive-payload"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let output = test_tracing::messages("se_manager::web::auth").join("\n");
        for required in [
            "operation=remote_boundary",
            "route_class=conversation",
            "capability=mutate",
            "provenance=public_tunnel",
            "stable_code=OK",
            "http_status=204",
            "duration_ms=",
        ] {
            assert!(output.contains(required), "missing {required}: {output}");
        }
        assert!(!output.contains(supplied_path));
        assert!(!output.contains("supplied-conversation-id"));
        assert!(!output.contains(TOKEN));
        assert!(!output.contains("supplied-sensitive-payload"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn boundary_log_global_harness_captures_500_parallel_calls_without_missing_fields() {
        let _boundary_log_test_guard = test_tracing::lock().await;
        let authority = Arc::new(authority());
        let app = axum::Router::new()
            .route("/conversations", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn(capability_middleware))
            .layer(Extension(RemoteRouteClass::Conversation))
            .layer(Extension(IngressProvenance::LocalOperator))
            .layer(Extension(authority));

        let mut tasks = Vec::with_capacity(500);
        for ordinal in 0..500_u16 {
            let app = app.clone();
            tasks.push(tokio::spawn(async move {
                app.oneshot(
                    Request::builder()
                        .uri("/conversations")
                        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                        .extension(ConnectInfo(SocketAddr::from((
                            [127, 0, 0, 1],
                            10_000_u16.saturating_add(ordinal),
                        ))))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap(), StatusCode::OK);
        }

        let boundary = test_tracing::messages("se_manager::web::auth")
            .into_iter()
            .filter(|message| message.contains("operation=remote_boundary"))
            .collect::<Vec<_>>();
        assert!(
            boundary.len() >= 500,
            "captured {} boundary records",
            boundary.len()
        );
        for message in boundary {
            for required in [
                "method=GET",
                "route_class=conversation",
                "capability=read",
                "provenance=local_operator",
                "stable_code=OK",
                "http_status=200",
                "duration_ms=",
            ] {
                assert!(message.contains(required), "missing {required}: {message}");
            }
        }
    }

    #[test]
    fn route_classes_and_boundary_logging_are_identifier_free() {
        for (path, expected) in [
            (
                "/conversations/conversation-secret/lifecycle/detach",
                RemoteRouteClass::Conversation,
            ),
            ("/projects/default", RemoteRouteClass::Project),
            ("/worktree/remove", RemoteRouteClass::Worktree),
            ("/cli-sessions", RemoteRouteClass::CliSession),
            ("/conversation-recovery/resolve", RemoteRouteClass::Recovery),
            ("/ws", RemoteRouteClass::AcpWebSocket),
            ("/terminal/ws", RemoteRouteClass::TerminalWebSocket),
        ] {
            assert_eq!(RemoteRouteClass::from_path(path), Some(expected));
            assert!(!expected.as_str().contains("secret"));
            assert!(!expected.as_str().contains('/'));
        }
        let source = include_str!("auth.rs");
        assert!(!source.contains(&["request_type", " = format!"].concat()));
        assert!(!source.contains(&["request_type", ","].concat()));
    }

    #[test]
    fn application_status_mapping_is_stable() {
        for (code, expected) in [
            ("VALIDATION_ERROR", StatusCode::BAD_REQUEST),
            ("UNAUTHORIZED", StatusCode::UNAUTHORIZED),
            ("FORBIDDEN", StatusCode::FORBIDDEN),
            ("CONVERSATION_NOT_FOUND", StatusCode::NOT_FOUND),
            ("CONVERSATION_CONFLICT", StatusCode::CONFLICT),
            (
                "CONVERSATION_RECOVERY_REQUIRED",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "CONVERSATION_DURABILITY_FAILED",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ] {
            assert_eq!(status_for_code(code), expected, "{code}");
        }
    }

    #[test]
    fn windows_token_descriptor_rejects_foreign_user_and_network_service_allow_ace() {
        let source = include_str!("auth.rs");
        assert!(
            source.contains("WinLocalSystemSid"),
            "allowlist must include WinLocalSystemSid S-1-5-18"
        );
        assert!(
            source.contains("S-1-5-18") || source.contains("WinLocalSystemSid"),
            "local system SID S-1-5-18 must be named"
        );
        assert!(
            source.contains("WinNetworkServiceSid") || source.contains("S-1-5-20"),
            "foreign test SID must be WinNetworkServiceSid S-1-5-20"
        );
        assert_eq!(test_tracing::WIN_LOCAL_SYSTEM_SID, "S-1-5-18");
        assert_eq!(test_tracing::WIN_NETWORK_SERVICE_SID, "S-1-5-20");
        assert!(
            source.contains("allowed.Mask == 0") || source.contains("allowed.Mask != 0"),
            "zero-mask ACEs stay ignored; nonzero foreign allow ACEs fail closed"
        );

        #[cfg(windows)]
        {
            use std::ffi::c_void;
            use std::mem::size_of;
            use std::ptr::null_mut;
            use windows_sys::Win32::Foundation::GENERIC_READ;
            use windows_sys::Win32::Security::{
                AddAccessAllowedAce, CreateWellKnownSid, InitializeAcl,
                WinBuiltinAdministratorsSid, WinNetworkServiceSid, ACL, ACL_REVISION,
            };

            fn sid(kind: i32) -> Vec<usize> {
                let mut storage = vec![0_usize; 16];
                let mut bytes = (storage.len() * size_of::<usize>()) as u32;
                assert_ne!(
                    unsafe {
                        CreateWellKnownSid(
                            kind,
                            null_mut(),
                            storage.as_mut_ptr().cast::<c_void>(),
                            &mut bytes,
                        )
                    },
                    0
                );
                storage
            }

            let owner = sid(WinBuiltinAdministratorsSid);
            let network = sid(WinNetworkServiceSid);
            let owner_ptr = owner.as_ptr().cast_mut().cast();
            let mut acl_storage = vec![0_usize; 128];
            let acl = acl_storage.as_mut_ptr().cast::<ACL>();
            assert_ne!(
                unsafe {
                    InitializeAcl(
                        acl,
                        (acl_storage.len() * size_of::<usize>()) as u32,
                        ACL_REVISION,
                    )
                },
                0
            );
            assert_ne!(
                unsafe {
                    AddAccessAllowedAce(
                        acl,
                        ACL_REVISION,
                        GENERIC_READ,
                        network.as_ptr().cast_mut().cast(),
                    )
                },
                0
            );
            assert_eq!(
                windows_token_file::validate_descriptor_for_tests(owner_ptr, acl, owner_ptr),
                Err(RemoteAuthError::Provisioning)
            );
        }
    }

    #[tokio::test]
    async fn capture_logger_scoped_id_does_not_contaminate_unrelated_or_post_guard_logs() {
        let scoped = test_tracing::lock_scoped("task-002-capture").await;
        let scoped_id = scoped.id();
        log::info!(target: "se_manager::web::auth", "scoped-capture-record");
        test_tracing::emit_unscoped_for_tests("se_manager::web::auth", "unrelated-concurrent-record");
        let scoped_messages = test_tracing::messages_for(scoped_id, "se_manager::web::auth");
        assert!(
            scoped_messages
                .iter()
                .any(|message| message.contains("scoped-capture-record")),
            "scoped capture must observe its own records: {scoped_messages:?}"
        );
        assert!(
            scoped_messages
                .iter()
                .all(|message| !message.contains("unrelated-concurrent-record")),
            "unscoped concurrent records must not contaminate the scoped bucket"
        );
        drop(scoped);

        log::info!(target: "se_manager::web::auth", "post-guard-observable-record");
        let forwarded = test_tracing::forwarded_messages("se_manager::web::auth");
        assert!(
            forwarded
                .iter()
                .any(|message| message.contains("post-guard-observable-record")),
            "post-guard logs must remain observable: {forwarded:?}"
        );
        assert!(
            test_tracing::messages_for(scoped_id, "se_manager::web::auth").is_empty(),
            "dropped scoped capture must not keep absorbing records"
        );
    }
}
