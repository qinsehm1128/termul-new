//! Host-owned, in-memory feedback. Callers hold a generation lease before invoking
//! this module; no runtime lock, plugin callback, disk I/O, or GUI work occurs here.
//! `now_ms` is a host clock (never plugin input), clamped against clock rollback.
use se_plugin_protocol::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const MAX_RECORDS: usize = 128;
const MAX_NOTIFICATIONS: usize = 8;
const MAX_PROGRESS: usize = 2;
const MAX_STATUS: usize = 2;
const TERMINAL_TTL_MS: u64 = 8_000;

/// A command resolved exclusively from host-owned records, not renderer arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackAction {
    pub command_id: String,
    pub args: Value,
}

/// Mutable only under its own short-lived service lock, outside the runtime lock.
pub struct FeedbackHost {
    ui: HostUiSnapshot,
    last_now: u64,
    rates: BTreeMap<(String, String), Rates>,
}

const MAX_RATE_OWNERS: usize = 128;

#[derive(Clone)]
struct Bucket {
    credit: u64,
    updated_at: u64,
}

impl Bucket {
    fn new(burst: u64, now: u64) -> Self {
        Self {
            credit: burst * 1000,
            updated_at: now,
        }
    }
    fn admit(&mut self, rate: u64, burst: u64, now: u64) -> Result<(), PluginError> {
        self.credit = self
            .credit
            .saturating_add(now.saturating_sub(self.updated_at).saturating_mul(rate))
            .min(burst * 1000);
        self.updated_at = now;
        if self.credit < 1000 {
            return Err(PluginError::new(PLUGIN_BUSY, "host feedback rate exceeded"));
        }
        self.credit -= 1000;
        Ok(())
    }
}

#[derive(Clone)]
struct Rates {
    feedback: Bucket,
    status: Bucket,
    published: bool,
}

impl FeedbackHost {
    /// `boot_id` is a host-generated UUID, shared with the other host services.
    pub fn new(boot_id: String) -> Self {
        Self {
            ui: HostUiSnapshot {
                schema_version: SCHEMA_VERSION,
                boot_id,
                revision: 0,
                notifications: Vec::new(),
                progress: Vec::new(),
                status_items: Vec::new(),
            },
            last_now: 0,
            rates: BTreeMap::new(),
        }
    }

    /// Reserve a hidden provisional generation. Quotas include its records, but
    /// snapshots and renderer actions cannot observe them until activation.
    pub fn begin(&mut self, plugin_id: &str, generation: &str) -> Result<(), PluginError> {
        if !is_plugin_id(plugin_id) || !is_uuid(generation) {
            return Err(invalid("invalid feedback owner"));
        }
        let key = (plugin_id.into(), generation.into());
        if self.rates.contains_key(&key) {
            return Err(PluginError::new(
                PLUGIN_BUSY,
                "feedback owner already registered",
            ));
        }
        if self.rates.len() >= MAX_RATE_OWNERS {
            return Err(quota());
        }
        self.rates.insert(
            key,
            Rates {
                feedback: Bucket::new(10, self.last_now),
                status: Bucket::new(20, self.last_now),
                published: false,
            },
        );
        Ok(())
    }

    /// Publish the already validated provisional records atomically.
    pub fn activate(&mut self, plugin_id: &str, generation: &str) -> Result<(), PluginError> {
        let revision = self.next_revision()?;
        let owner = self
            .rates
            .get_mut(&(plugin_id.into(), generation.into()))
            .ok_or_else(stale)?;
        if !owner.published {
            owner.published = true;
            self.ui.revision = revision;
        }
        Ok(())
    }

    fn published(&self, plugin_id: &str, generation: &str) -> bool {
        self.rates
            .get(&(plugin_id.into(), generation.into()))
            .is_some_and(|owner| owner.published)
    }

    fn advance(&mut self, now_ms: u64) -> u64 {
        self.last_now = self.last_now.max(now_ms.min(MAX_SAFE_INTEGER - 30_000));
        self.last_now
    }

    fn next_revision(&self) -> Result<u64, PluginError> {
        self.ui
            .revision
            .checked_add(1)
            .filter(|r| *r <= MAX_SAFE_INTEGER)
            .ok_or_else(quota)
    }

    fn expire(&mut self, now_ms: u64) -> u64 {
        let now = self.advance(now_ms);
        let before = self.ui.notifications.len() + self.ui.progress.len();
        self.ui
            .notifications
            .retain(|n| n.expires_at.is_none_or(|t| t > now));
        self.ui
            .progress
            .retain(|p| p.expires_at.is_none_or(|t| t > now));
        if before != self.ui.notifications.len() + self.ui.progress.len() {
            self.ui.revision = self.ui.revision.saturating_add(1).min(MAX_SAFE_INTEGER);
        }
        now
    }

    fn admit(
        &mut self,
        plugin_id: &str,
        generation: &str,
        status: bool,
        now: u64,
    ) -> Result<(), PluginError> {
        let key = (plugin_id.into(), generation.into());
        let rates = self.rates.get_mut(&key).ok_or_else(stale)?;
        if status {
            rates.status.admit(10, 20, now)
        } else {
            rates.feedback.admit(5, 10, now)
        }
    }

    fn commit(&mut self, mut candidate: HostUiSnapshot) -> Result<(), PluginError> {
        candidate.revision = self.next_revision()?;
        if candidate.notifications.len() + candidate.progress.len() + candidate.status_items.len()
            > MAX_RECORDS
        {
            return Err(quota());
        }
        // Also checks UTF-8 JSON size, action argument sizes, metadata and DTOs.
        validate_host_ui_snapshot(&candidate).map_err(|_| quota())?;
        self.ui = candidate;
        Ok(())
    }

    /// Dispatch only the frozen feedback/status methods. The caller authorizes
    /// the service and active generation before passing its trusted manifest.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        manifest: &Manifest,
        generation: &str,
        active_project_id: Option<&str>,
        service: &str,
        method: &str,
        input: &Value,
        now_ms: u64,
    ) -> Result<Value, PluginError> {
        if !is_uuid(generation) || !is_plugin_id(&manifest.id) {
            return Err(invalid("invalid feedback attribution"));
        }
        let bytes = serde_json::to_vec(input).map_err(|_| invalid("invalid feedback input"))?;
        let now = self.expire(now_ms);
        match (service, method) {
            ("se.host.feedback", "notification.upsert") => {
                let request = parse_notification_upsert_request(&bytes)?;
                self.admit(&manifest.id, generation, false, now)?;
                self.notification_upsert(manifest, generation, request, now)
            }
            ("se.host.feedback", "notification.close") => {
                let request = parse_notification_close_request(&bytes)?;
                self.admit(&manifest.id, generation, false, now)?;
                let mut candidate = self.ui.clone();
                candidate.notifications.retain(|n| {
                    !(n.plugin_id == manifest.id
                        && n.generation == generation
                        && n.id == request.id)
                });
                if candidate.notifications.len() == self.ui.notifications.len() {
                    return Ok(json!({"closed": true}));
                }
                self.commit(candidate)?;
                Ok(json!({"closed": true}))
            }
            ("se.host.feedback", "progress.upsert") => {
                let request = parse_progress_upsert_request(&bytes)?;
                self.admit(&manifest.id, generation, false, now)?;
                self.progress_upsert(manifest, generation, request, now)
            }
            ("se.host.status", "set") => {
                let request = parse_status_set_request(&bytes)?;
                self.admit(&manifest.id, generation, true, now)?;
                self.status_set(manifest, generation, active_project_id, request, now)
            }
            ("se.host.status", "clear") => {
                let request = parse_status_clear_request(&bytes)?;
                self.admit(&manifest.id, generation, true, now)?;
                if !manifest
                    .contributes
                    .status_items
                    .iter()
                    .any(|s| s.id == request.id)
                {
                    return Err(invalid("status item is not declared"));
                }
                let mut candidate = self.ui.clone();
                candidate.status_items.retain(|s| {
                    !(s.plugin_id == manifest.id
                        && s.generation == generation
                        && s.id == request.id)
                });
                if candidate.status_items.len() == self.ui.status_items.len() {
                    return Ok(json!({"cleared": true}));
                }
                self.commit(candidate)?;
                Ok(json!({"cleared": true}))
            }
            _ => Err(PluginError::new(
                PLUGIN_NOT_SUPPORTED,
                "unsupported feedback method",
            )),
        }
    }

    fn notification_upsert(
        &mut self,
        manifest: &Manifest,
        generation: &str,
        request: NotificationUpsertRequest,
        now: u64,
    ) -> Result<Value, PluginError> {
        if request
            .actions
            .iter()
            .any(|a| !has_command(manifest, &a.command_id))
        {
            return Err(invalid("notification command is not declared"));
        }
        let mut candidate = self.ui.clone();
        let existing = candidate.notifications.iter().position(|n| {
            n.plugin_id == manifest.id && n.generation == generation && n.id == request.id
        });
        if existing.is_none()
            && candidate
                .notifications
                .iter()
                .filter(|n| n.plugin_id == manifest.id && n.generation == generation)
                .count()
                >= MAX_NOTIFICATIONS
        {
            return Err(quota());
        }
        let record = HostNotificationRecord {
            plugin_id: manifest.id.clone(),
            plugin_name: manifest.name.clone(),
            generation: generation.into(),
            revision: self.next_revision()?,
            created_at: existing.map_or(now, |i| candidate.notifications[i].created_at),
            updated_at: now,
            expires_at: Some(now + u64::from(request.ttl_ms)),
            id: request.id.clone(),
            severity: request.severity,
            title: request.title,
            message: request.message,
            actions: request.actions,
            ttl_ms: request.ttl_ms,
        };
        if let Some(i) = existing {
            candidate.notifications[i] = record;
        } else {
            candidate.notifications.push(record);
        }
        self.commit(candidate)?;
        Ok(json!({"accepted": true, "id": request.id}))
    }

    fn progress_upsert(
        &mut self,
        manifest: &Manifest,
        generation: &str,
        request: ProgressUpsertRequest,
        now: u64,
    ) -> Result<Value, PluginError> {
        if request
            .cancel_command_id
            .as_deref()
            .is_some_and(|id| !has_command(manifest, id))
        {
            return Err(invalid("progress cancel command is not declared"));
        }
        let mut candidate = self.ui.clone();
        let existing = candidate.progress.iter().position(|p| {
            p.plugin_id == manifest.id && p.generation == generation && p.id == request.id
        });
        if existing.is_none()
            && candidate
                .progress
                .iter()
                .filter(|p| p.plugin_id == manifest.id && p.generation == generation)
                .count()
                >= MAX_PROGRESS
        {
            return Err(quota());
        }
        let record = HostProgressRecord {
            plugin_id: manifest.id.clone(),
            plugin_name: manifest.name.clone(),
            generation: generation.into(),
            revision: self.next_revision()?,
            created_at: existing.map_or(now, |i| candidate.progress[i].created_at),
            updated_at: now,
            expires_at: (request.state != ProgressState::Running).then_some(now + TERMINAL_TTL_MS),
            id: request.id.clone(),
            title: request.title,
            state: request.state,
            fraction: request.fraction,
            cancel_command_id: request.cancel_command_id,
        };
        if let Some(i) = existing {
            candidate.progress[i] = record;
        } else {
            candidate.progress.push(record);
        }
        self.commit(candidate)?;
        Ok(json!({"accepted": true, "id": request.id}))
    }

    fn status_set(
        &mut self,
        manifest: &Manifest,
        generation: &str,
        active_project_id: Option<&str>,
        request: StatusSetRequest,
        now: u64,
    ) -> Result<Value, PluginError> {
        let declaration = manifest
            .contributes
            .status_items
            .iter()
            .find(|s| s.id == request.id)
            .ok_or_else(|| invalid("status item is not declared"))?;
        match declaration.scope {
            HostScope::Window if request.project_id.is_some() => {
                return Err(invalid("window status cannot have a project"))
            }
            HostScope::ActiveProject
                if request.project_id.is_none()
                    || request.project_id.as_deref() != active_project_id =>
            {
                return Err(invalid("status project is not active"))
            }
            _ => {}
        }
        if declaration
            .command_id
            .as_deref()
            .is_some_and(|id| !has_command(manifest, id))
        {
            return Err(invalid("status command is not declared"));
        }
        let mut candidate = self.ui.clone();
        let existing = candidate.status_items.iter().position(|s| {
            s.plugin_id == manifest.id && s.generation == generation && s.id == request.id
        });
        if existing.is_none()
            && candidate
                .status_items
                .iter()
                .filter(|s| s.plugin_id == manifest.id && s.generation == generation)
                .count()
                >= MAX_STATUS
        {
            return Err(quota());
        }
        let record = HostStatusRecord {
            plugin_id: manifest.id.clone(),
            plugin_name: manifest.name.clone(),
            generation: generation.into(),
            revision: self.next_revision()?,
            created_at: existing.map_or(now, |i| candidate.status_items[i].created_at),
            updated_at: now,
            expires_at: None,
            id: request.id.clone(),
            label: declaration.label.clone(),
            location: StatusLocation::StatusBar,
            scope: declaration.scope,
            command_id: declaration.command_id.clone(),
            order: declaration.order,
            text: request.text,
            icon: request.icon,
            tone: request.tone,
            tooltip: request.tooltip,
            visible: request.visible,
            project_id: request.project_id,
        };
        if let Some(i) = existing {
            candidate.status_items[i] = record;
        } else {
            candidate.status_items.push(record);
        }
        self.commit(candidate)?;
        Ok(json!({"accepted": true, "id": request.id}))
    }

    pub fn snapshot(&mut self, now_ms: u64) -> HostUiSnapshot {
        self.expire(now_ms);
        let mut snapshot = self.ui.clone();
        snapshot
            .notifications
            .retain(|n| self.published(&n.plugin_id, &n.generation));
        snapshot
            .progress
            .retain(|p| self.published(&p.plugin_id, &p.generation));
        snapshot
            .status_items
            .retain(|s| self.published(&s.plugin_id, &s.generation));
        snapshot
    }

    pub fn snapshot_for_project(
        &mut self,
        now_ms: u64,
        active_project_id: Option<&str>,
    ) -> HostUiSnapshot {
        self.scoped_snapshot(active_project_id, now_ms)
    }

    /// Filter active-project status at presentation time; changing projects does
    /// not discard the last value for the plugin's declared slot.
    pub fn scoped_snapshot(
        &mut self,
        active_project_id: Option<&str>,
        now_ms: u64,
    ) -> HostUiSnapshot {
        let mut snapshot = self.snapshot(now_ms);
        snapshot.status_items.retain(|s| {
            s.visible
                && (s.scope == HostScope::Window || s.project_id.as_deref() == active_project_id)
        });
        snapshot
    }

    pub fn dismiss(
        &mut self,
        request: &HostUiDismissRequest,
        now_ms: u64,
    ) -> Result<HostUiDismissResult, PluginError> {
        validate_host_ui_dismiss_request(request)?;
        self.expire(now_ms);
        if !self.published(&request.plugin_id, &request.generation) {
            return Err(stale());
        }
        let mut candidate = self.ui.clone();
        let removed = match request.kind {
            HostUiKind::Notification => {
                let before = candidate.notifications.len();
                candidate.notifications.retain(|n| {
                    !(n.plugin_id == request.plugin_id
                        && n.generation == request.generation
                        && n.id == request.id)
                });
                before != candidate.notifications.len()
            }
            HostUiKind::Progress => {
                let before = candidate.progress.len();
                candidate.progress.retain(|p| {
                    !(p.plugin_id == request.plugin_id
                        && p.generation == request.generation
                        && p.id == request.id)
                });
                before != candidate.progress.len()
            }
        };
        if !removed {
            return Err(action_error());
        }
        self.commit(candidate)?;
        Ok(HostUiDismissResult { dismissed: true })
    }

    /// Resolve only feedback contribution kinds. The outer host validates the
    /// context revision/target and retains its generation lease through enqueue.
    pub fn resolve_action(
        &mut self,
        manifest: &Manifest,
        generation: &str,
        request: &ExecuteContributionRequest,
        active_project_id: Option<&str>,
        now_ms: u64,
    ) -> Result<FeedbackAction, PluginError> {
        validate_execute_contribution_request(request)?;
        self.expire(now_ms);
        if request.plugin_id != manifest.id
            || request.generation != generation
            || !self.published(&manifest.id, generation)
        {
            return Err(stale());
        }
        let action = match request.kind {
            ContributionKind::Notification => {
                let record = self
                    .ui
                    .notifications
                    .iter()
                    .find(|n| {
                        n.plugin_id == manifest.id
                            && n.generation == generation
                            && n.id == request.contribution_id
                            && Some(n.revision) == request.record_revision
                    })
                    .ok_or_else(action_error)?;
                let action = record
                    .actions
                    .get(request.action_index.ok_or_else(action_error)? as usize)
                    .ok_or_else(action_error)?;
                FeedbackAction {
                    command_id: action.command_id.clone(),
                    args: action.args.clone(),
                }
            }
            ContributionKind::Progress => {
                let record = self
                    .ui
                    .progress
                    .iter()
                    .find(|p| {
                        p.plugin_id == manifest.id
                            && p.generation == generation
                            && p.id == request.contribution_id
                            && Some(p.revision) == request.record_revision
                            && p.state == ProgressState::Running
                    })
                    .ok_or_else(action_error)?;
                FeedbackAction {
                    command_id: record.cancel_command_id.clone().ok_or_else(action_error)?,
                    args: Value::Null,
                }
            }
            ContributionKind::Status => {
                let record = self
                    .ui
                    .status_items
                    .iter()
                    .find(|s| {
                        s.plugin_id == manifest.id
                            && s.generation == generation
                            && s.id == request.contribution_id
                            && s.visible
                    })
                    .ok_or_else(action_error)?;
                if record.scope == HostScope::ActiveProject
                    && (record.project_id.as_deref() != active_project_id
                        || record.project_id != request.target.project_id)
                {
                    return Err(action_error());
                }
                let declaration = manifest
                    .contributes
                    .status_items
                    .iter()
                    .find(|s| s.id == record.id)
                    .ok_or_else(action_error)?;
                if declaration.command_id != record.command_id || declaration.scope != record.scope
                {
                    return Err(action_error());
                }
                FeedbackAction {
                    command_id: record.command_id.clone().ok_or_else(action_error)?,
                    args: Value::Null,
                }
            }
            _ => return Err(action_error()),
        };
        if !has_command(manifest, &action.command_id) {
            return Err(action_error());
        }
        Ok(action)
    }

    /// Alias used by the generation authority during runtime revocation.
    pub fn revoke(&mut self, plugin_id: &str, generation: &str) {
        self.clear_generation(plugin_id, generation);
    }

    /// Drop transient UI state when the owning renderer detaches.
    pub fn clear(&mut self) {
        let had_records = !self.ui.notifications.is_empty()
            || !self.ui.progress.is_empty()
            || !self.ui.status_items.is_empty();
        self.ui.notifications.clear();
        self.ui.progress.clear();
        self.ui.status_items.clear();
        // Source detach clears transient records, not plugin generation leases.
        // Their rate buckets must survive to prevent detach/refill abuse.
        if had_records {
            self.ui.revision = self.ui.revision.saturating_add(1).min(MAX_SAFE_INTEGER);
        }
    }

    /// Revoke all transient records and rate state associated with this lease.
    pub fn clear_generation(&mut self, plugin_id: &str, generation: &str) {
        let before =
            self.ui.notifications.len() + self.ui.progress.len() + self.ui.status_items.len();
        self.ui
            .notifications
            .retain(|n| n.plugin_id != plugin_id || n.generation != generation);
        self.ui
            .progress
            .retain(|p| p.plugin_id != plugin_id || p.generation != generation);
        self.ui
            .status_items
            .retain(|s| s.plugin_id != plugin_id || s.generation != generation);
        self.rates.remove(&(plugin_id.into(), generation.into()));
        if before
            != self.ui.notifications.len() + self.ui.progress.len() + self.ui.status_items.len()
        {
            self.ui.revision = self.ui.revision.saturating_add(1).min(MAX_SAFE_INTEGER);
        }
    }
}

fn invalid(message: &str) -> PluginError {
    PluginError::new(PLUGIN_INVALID_REQUEST, message)
}
fn stale() -> PluginError {
    PluginError::new(PLUGIN_STALE_GENERATION, "feedback generation is stale")
}
fn action_error() -> PluginError {
    PluginError::new(
        PLUGIN_INVALID_ACTION,
        "feedback action is no longer available",
    )
}
fn quota() -> PluginError {
    PluginError::new(PLUGIN_QUOTA_EXCEEDED, "host feedback quota exceeded")
}
fn has_command(manifest: &Manifest, id: &str) -> bool {
    manifest.contributes.commands.iter().any(|c| c.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    const BOOT: &str = "11111111-1111-4111-8111-111111111111";
    const GEN: &str = "22222222-2222-4222-8222-222222222222";
    const GEN2: &str = "33333333-3333-4333-8333-333333333333";

    fn manifest(id: &str) -> Manifest {
        serde_json::from_value(json!({
            "schemaVersion": 1, "id": id, "name": "Feedback plugin", "version": "1.0.0", "abi": {"min":1,"max":1},
            "services":{"provides":[],"requires":[]},
            "contributes": {
                "configuration":{"title":"Feedback","properties":{}}, "views":[],
                "commands": [{"id":"run", "title":"Run"}],
                "statusItems": [
                    {"id":"window", "label":"Window", "location":"statusBar", "scope":"window", "commandId":"run"},
                    {"id":"project", "label":"Project", "location":"statusBar", "scope":"activeProject", "commandId":"run"}
                ]
            }
        })).unwrap()
    }
    fn notification(id: &str) -> Value {
        json!({"id":id, "severity":"info", "title":"Title", "message":"Message", "ttlMs":2000,
            "actions":[{"label":"Run", "commandId":"run", "args":{"trusted":true}}]})
    }
    fn progress(id: &str, state: &str) -> Value {
        json!({"id":id,"title":"Progress","state":state,"cancelCommandId":"run"})
    }
    fn status(id: &str) -> Value {
        json!({"id":id,"text":"Ready","tone":"default","visible":true})
    }
    fn action(kind: ContributionKind, id: &str) -> ExecuteContributionRequest {
        ExecuteContributionRequest {
            plugin_id: "sample.feedback".into(),
            generation: GEN.into(),
            context_revision: 1,
            kind,
            contribution_id: id.into(),
            target: ContributionTarget {
                project_id: None,
                terminal_id: None,
                session_id: None,
            },
            action_index: (kind == ContributionKind::Notification).then_some(0),
            record_revision: matches!(
                kind,
                ContributionKind::Notification | ContributionKind::Progress
            )
            .then_some(1),
        }
    }

    #[test]
    fn notification_upsert_caps_ttl_and_monotonic_metadata() {
        let mut host = FeedbackHost::new(BOOT.into());
        let m = manifest("sample.feedback");
        host.begin(&m.id, GEN).unwrap();
        host.activate(&m.id, GEN).unwrap();
        for i in 0..8 {
            host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "notification.upsert",
                &notification(&format!("n{i}")),
                100,
            )
            .unwrap();
        }
        assert_eq!(
            host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "notification.upsert",
                &notification("overflow"),
                100
            )
            .unwrap_err()
            .code,
            PLUGIN_QUOTA_EXCEEDED
        );
        let revision = host.ui.revision;
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "notification.upsert",
            &notification("n7"),
            1,
        )
        .unwrap();
        let record = host.ui.notifications.iter().find(|n| n.id == "n7").unwrap();
        assert_eq!(record.updated_at, 100);
        assert_eq!(record.revision, revision + 1);
        assert_eq!(record.plugin_name, m.name);
        assert_eq!(record.generation, GEN);
        assert!(host.snapshot(2100).notifications.is_empty());
    }

    #[test]
    fn progress_cap_terminal_expiry_and_running_persistence() {
        let mut host = FeedbackHost::new(BOOT.into());
        let m = manifest("sample.feedback");
        host.begin(&m.id, GEN).unwrap();
        host.activate(&m.id, GEN).unwrap();
        for id in ["one", "two"] {
            host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "progress.upsert",
                &progress(id, "running"),
                100,
            )
            .unwrap();
        }
        assert_eq!(
            host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "progress.upsert",
                &progress("three", "running"),
                100
            )
            .unwrap_err()
            .code,
            PLUGIN_QUOTA_EXCEEDED
        );
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "progress.upsert",
            &progress("one", "completed"),
            100,
        )
        .unwrap();
        assert_eq!(host.snapshot(100 + TERMINAL_TTL_MS - 1).progress.len(), 2);
        assert_eq!(host.snapshot(100 + TERMINAL_TTL_MS).progress.len(), 1);
        assert_eq!(host.snapshot(100 + 300_000).progress.len(), 1);
        assert_eq!(host.ui.progress[0].expires_at, None);
    }

    #[test]
    fn notification_commands_and_expired_dismissal_fail_closed() {
        let mut host = FeedbackHost::new(BOOT.into());
        let m = manifest("sample.feedback");
        host.begin(&m.id, GEN).unwrap();
        host.activate(&m.id, GEN).unwrap();
        let mut invalid = notification("bad");
        invalid["actions"][0]["commandId"] = json!("undeclared");
        assert!(host
            .apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "notification.upsert",
                &invalid,
                0
            )
            .is_err());
        let mut invalid = progress("bad", "running");
        invalid["cancelCommandId"] = json!("undeclared");
        assert!(host
            .apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "progress.upsert",
                &invalid,
                0
            )
            .is_err());
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "notification.upsert",
            &notification("note"),
            0,
        )
        .unwrap();
        let mut request = action(ContributionKind::Notification, "note");
        request.record_revision = Some(
            host.ui
                .notifications
                .iter()
                .find(|n| n.id == "note")
                .unwrap()
                .revision,
        );
        assert_eq!(
            host.resolve_action(&m, GEN, &request, None, 1)
                .unwrap()
                .args,
            json!({"trusted":true})
        );
        assert_eq!(
            host.resolve_action(&m, GEN2, &request, None, 1)
                .unwrap_err()
                .code,
            PLUGIN_STALE_GENERATION
        );
        assert!(host.resolve_action(&m, GEN, &request, None, 2000).is_err());
        let dismiss = HostUiDismissRequest {
            plugin_id: m.id.clone(),
            generation: GEN.into(),
            kind: HostUiKind::Notification,
            id: "note".into(),
        };
        assert!(host.dismiss(&dismiss, 2000).is_err());
    }

    #[test]
    fn status_declarations_scope_visibility_and_revocation() {
        let mut host = FeedbackHost::new(BOOT.into());
        let m = manifest("sample.feedback");
        host.begin(&m.id, GEN).unwrap();
        host.activate(&m.id, GEN).unwrap();
        assert!(host
            .apply(
                &m,
                GEN,
                None,
                "se.host.status",
                "set",
                &status("unknown"),
                0
            )
            .is_err());
        assert!(host
            .apply(
                &m,
                GEN,
                None,
                "se.host.status",
                "set",
                &status("project"),
                0
            )
            .is_err());
        let mut project = status("project");
        project["projectId"] = json!("p1");
        assert!(host
            .apply(&m, GEN, Some("p2"), "se.host.status", "set", &project, 0)
            .is_err());
        host.apply(&m, GEN, Some("p1"), "se.host.status", "set", &project, 0)
            .unwrap();
        assert_eq!(host.scoped_snapshot(Some("p1"), 0).status_items.len(), 1);
        assert!(host.scoped_snapshot(Some("p2"), 0).status_items.is_empty());
        assert_eq!(host.scoped_snapshot(Some("p1"), 0).status_items.len(), 1);
        let mut request = action(ContributionKind::Status, "project");
        request.target.project_id = Some("p1".into());
        assert_eq!(
            host.resolve_action(&m, GEN, &request, Some("p1"), 1)
                .unwrap()
                .command_id,
            "run"
        );
        assert!(host
            .resolve_action(&m, GEN, &request, Some("p2"), 1)
            .is_err());
        project["visible"] = json!(false);
        host.apply(&m, GEN, Some("p1"), "se.host.status", "set", &project, 1)
            .unwrap();
        assert!(host
            .resolve_action(&m, GEN, &request, Some("p1"), 1)
            .is_err());
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "notification.upsert",
            &notification("note"),
            1,
        )
        .unwrap();
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "progress.upsert",
            &progress("job", "running"),
            1,
        )
        .unwrap();
        host.clear_generation(&m.id, GEN);
        let snapshot = host.snapshot(2);
        assert!(
            snapshot.notifications.is_empty()
                && snapshot.progress.is_empty()
                && snapshot.status_items.is_empty()
        );
    }

    #[test]
    fn rate_bursts_refill_and_service_buckets_are_independent() {
        let mut host = FeedbackHost::new(BOOT.into());
        let m = manifest("sample.feedback");
        host.begin(&m.id, GEN).unwrap();
        host.activate(&m.id, GEN).unwrap();
        for _ in 0..10 {
            host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "notification.upsert",
                &notification("note"),
                0,
            )
            .unwrap();
        }
        assert_eq!(
            host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "progress.upsert",
                &progress("job", "running"),
                199
            )
            .unwrap_err()
            .code,
            PLUGIN_BUSY
        );
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "progress.upsert",
            &progress("job", "running"),
            200,
        )
        .unwrap();
        for _ in 0..20 {
            host.apply(
                &m,
                GEN,
                None,
                "se.host.status",
                "set",
                &status("window"),
                200,
            )
            .unwrap();
        }
        assert_eq!(
            host.apply(
                &m,
                GEN,
                None,
                "se.host.status",
                "set",
                &status("window"),
                299
            )
            .unwrap_err()
            .code,
            PLUGIN_BUSY
        );
        host.apply(
            &m,
            GEN,
            None,
            "se.host.status",
            "set",
            &status("window"),
            300,
        )
        .unwrap();
        assert_eq!(
            host.apply(&m, GEN, None, "se.host.status", "set", &status("window"), 1)
                .unwrap_err()
                .code,
            PLUGIN_BUSY
        );
        host.revoke(&m.id, GEN);
        host.begin(&m.id, GEN2).unwrap();
        host.activate(&m.id, GEN2).unwrap();
        host.apply(
            &m,
            GEN2,
            None,
            "se.host.feedback",
            "notification.upsert",
            &notification("note"),
            300,
        )
        .unwrap();
        assert_eq!(host.rates.len(), 1);
    }

    #[test]
    fn close_and_clear_are_idempotent_and_dismiss_is_generation_fenced() {
        let mut host = FeedbackHost::new(BOOT.into());
        let m = manifest("sample.feedback");
        host.begin(&m.id, GEN).unwrap();
        host.activate(&m.id, GEN).unwrap();
        for _ in 0..2 {
            assert_eq!(
                host.apply(
                    &m,
                    GEN,
                    None,
                    "se.host.feedback",
                    "notification.close",
                    &json!({"id":"absent"}),
                    0
                )
                .unwrap(),
                json!({"closed":true})
            );
            assert_eq!(
                host.apply(
                    &m,
                    GEN,
                    None,
                    "se.host.status",
                    "clear",
                    &json!({"id":"window"}),
                    0
                )
                .unwrap(),
                json!({"cleared":true})
            );
        }
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "notification.upsert",
            &notification("note"),
            0,
        )
        .unwrap();
        let mut dismiss = HostUiDismissRequest {
            plugin_id: m.id.clone(),
            generation: GEN2.into(),
            kind: HostUiKind::Notification,
            id: "note".into(),
        };
        assert!(host.dismiss(&dismiss, 0).is_err());
        assert_eq!(host.snapshot(0).notifications.len(), 1);
        dismiss.generation = GEN.into();
        host.dismiss(&dismiss, 0).unwrap();
        assert!(host
            .resolve_action(
                &m,
                GEN,
                &action(ContributionKind::Notification, "note"),
                None,
                0
            )
            .is_err());
    }

    #[test]
    fn provisional_state_is_hidden_until_activation_and_revoke_fails_closed() {
        let mut host = FeedbackHost::new(BOOT.into());
        let m = manifest("sample.feedback");
        assert_eq!(
            host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "notification.upsert",
                &notification("note"),
                0
            )
            .unwrap_err()
            .code,
            PLUGIN_STALE_GENERATION
        );
        host.begin(&m.id, GEN).unwrap();
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "notification.upsert",
            &notification("note"),
            0,
        )
        .unwrap();
        assert!(host.snapshot(0).notifications.is_empty());
        let mut request = action(ContributionKind::Notification, "note");
        request.record_revision = Some(
            host.ui
                .notifications
                .iter()
                .find(|n| n.id == "note")
                .unwrap()
                .revision,
        );
        assert!(host.resolve_action(&m, GEN, &request, None, 0).is_err());
        host.activate(&m.id, GEN).unwrap();
        assert_eq!(host.snapshot(0).notifications.len(), 1);
        assert!(host.resolve_action(&m, GEN, &request, None, 0).is_ok());
        host.revoke(&m.id, GEN);
        assert!(host.snapshot(0).notifications.is_empty());
        assert_eq!(
            host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "notification.upsert",
                &notification("note"),
                0
            )
            .unwrap_err()
            .code,
            PLUGIN_STALE_GENERATION
        );
        assert!(host.activate(&m.id, GEN).is_err());
        assert!(host.resolve_action(&m, GEN, &request, None, 0).is_err());
    }

    #[test]
    fn stale_feedback_revisions_cannot_execute_replacement_actions() {
        for kind in [ContributionKind::Notification, ContributionKind::Progress] {
            let mut host = FeedbackHost::new(BOOT.into());
            let m = manifest("sample.feedback");
            host.begin(&m.id, GEN).unwrap();
            host.activate(&m.id, GEN).unwrap();
            let (method, mut value) = if kind == ContributionKind::Notification {
                ("notification.upsert", notification("record"))
            } else {
                ("progress.upsert", progress("record", "running"))
            };
            host.apply(&m, GEN, None, "se.host.feedback", method, &value, 0)
                .unwrap();
            let mut request = action(kind, "record");
            request.record_revision = Some(host.ui.revision);
            assert!(host.resolve_action(&m, GEN, &request, None, 0).is_ok());
            // A new action can occupy exactly the same plugin/generation/id.
            if kind == ContributionKind::Notification {
                value["actions"][0]["args"] = json!({"replacement":true});
            } else {
                value["title"] = json!("replacement progress");
            }
            host.apply(&m, GEN, None, "se.host.feedback", method, &value, 1)
                .unwrap();
            assert_eq!(
                host.resolve_action(&m, GEN, &request, None, 1)
                    .unwrap_err()
                    .code,
                PLUGIN_INVALID_ACTION
            );
            request.record_revision = Some(host.ui.revision);
            let resolved = host.resolve_action(&m, GEN, &request, None, 1).unwrap();
            assert_eq!(
                resolved.args,
                if kind == ContributionKind::Notification {
                    json!({"replacement":true})
                } else {
                    Value::Null
                }
            );
        }
    }

    #[test]
    fn status_cap_terminal_cancel_and_source_clear() {
        let mut host = FeedbackHost::new(BOOT.into());
        let mut m = manifest("sample.feedback");
        let mut third = m.contributes.status_items[0].clone();
        third.id = "third".into();
        m.contributes.status_items.push(third);
        host.begin(&m.id, GEN).unwrap();
        host.activate(&m.id, GEN).unwrap();
        host.apply(&m, GEN, None, "se.host.status", "set", &status("window"), 0)
            .unwrap();
        let mut project = status("project");
        project["projectId"] = json!("p1");
        host.apply(&m, GEN, Some("p1"), "se.host.status", "set", &project, 0)
            .unwrap();
        assert_eq!(
            host.apply(&m, GEN, None, "se.host.status", "set", &status("third"), 0)
                .unwrap_err()
                .code,
            PLUGIN_QUOTA_EXCEEDED
        );
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "progress.upsert",
            &progress("job", "running"),
            0,
        )
        .unwrap();
        let mut request = action(ContributionKind::Progress, "job");
        request.record_revision = Some(
            host.ui
                .progress
                .iter()
                .find(|p| p.id == "job")
                .unwrap()
                .revision,
        );
        assert_eq!(
            host.resolve_action(&m, GEN, &request, None, 0)
                .unwrap()
                .command_id,
            "run"
        );
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "progress.upsert",
            &progress("job", "completed"),
            0,
        )
        .unwrap();
        assert_eq!(
            host.resolve_action(&m, GEN, &request, None, 0)
                .unwrap_err()
                .code,
            PLUGIN_INVALID_ACTION
        );
        for ttl in [1999, 30001] {
            let mut n = notification("note");
            n["ttlMs"] = json!(ttl);
            assert_eq!(
                host.apply(
                    &m,
                    GEN,
                    None,
                    "se.host.feedback",
                    "notification.upsert",
                    &n,
                    0
                )
                .unwrap_err()
                .code,
                PLUGIN_INVALID_REQUEST
            );
        }
        host.clear();
        assert!(host.snapshot(0).status_items.is_empty());
        assert!(host.snapshot(0).progress.is_empty());
        host.apply(
            &m,
            GEN,
            None,
            "se.host.feedback",
            "notification.upsert",
            &notification("fresh"),
            0,
        )
        .unwrap();
        assert_eq!(host.snapshot(0).notifications.len(), 1);
    }

    #[test]
    fn total_record_and_serialized_byte_limits_are_transactional() {
        let mut host = FeedbackHost::new(BOOT.into());
        for i in 0..64 {
            let m = manifest(&format!("sample.p{i}"));
            host.begin(&m.id, GEN).unwrap();
            host.activate(&m.id, GEN).unwrap();
            host.apply(&m, GEN, None, "se.host.status", "set", &status("window"), 0)
                .unwrap();
            host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "progress.upsert",
                &progress("job", "running"),
                0,
            )
            .unwrap();
        }
        let before = host.snapshot(0);
        host.begin("sample.overflow", GEN).unwrap();
        host.activate("sample.overflow", GEN).unwrap();
        let revision = host.ui.revision;
        assert_eq!(
            host.apply(
                &manifest("sample.overflow"),
                GEN,
                None,
                "se.host.status",
                "set",
                &status("window"),
                0
            )
            .unwrap_err()
            .code,
            PLUGIN_QUOTA_EXCEEDED
        );
        assert_eq!(host.snapshot(0).status_items, before.status_items);
        assert_eq!(host.ui.revision, revision);
        let mut host = FeedbackHost::new(BOOT.into());
        let mut accepted = 0;
        for i in 0..128 {
            let m = manifest(&format!("sample.p{i}"));
            host.begin(&m.id, GEN).unwrap();
            host.activate(&m.id, GEN).unwrap();
            let mut n = notification("note");
            n["message"] = json!("x".repeat(1024));
            match host.apply(
                &m,
                GEN,
                None,
                "se.host.feedback",
                "notification.upsert",
                &n,
                0,
            ) {
                Ok(_) => accepted += 1,
                Err(e) => {
                    assert_eq!(e.code, PLUGIN_QUOTA_EXCEEDED);
                    break;
                }
            }
        }
        assert!(accepted > 0 && accepted < 128);
        let snapshot = host.snapshot(0);
        assert!(serde_json::to_vec(&snapshot).unwrap().len() <= 128 * 1024);
    }
}
