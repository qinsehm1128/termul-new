//! Reserved services own independent bounded memory; never enter a native worker here.
use crate::{
    host_context::HostContext,
    host_feedback::FeedbackHost,
    storage::{Storage, StorageRequest},
};
use se_plugin_protocol::*;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};
type Result<T> = std::result::Result<T, PluginError>;
struct Lease {
    manifest: Manifest,
    generation: String,
    published: bool,
    inflight: bool,
    tokens: f64,
    last_refill: u64,
}
struct ServicesState {
    leases: BTreeMap<String, Lease>,
    context: HostContext,
    feedback: FeedbackHost,
    last_ui_revision: u64,
    ui_revision: u64,
    feedback_revision: u64,
    active_project: Option<String>,
    source_live: bool,
    last_ui_emit: u64,
}
pub(crate) struct HostServices {
    state: Mutex<ServicesState>,
    storage: Storage,
}
fn err(code: &str, message: &str) -> PluginError {
    PluginError::new(code, message)
}
fn busy() -> PluginError {
    err(PLUGIN_BUSY, "host service state is busy")
}
fn encode<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|_| err(PLUGIN_INVALID_REQUEST, "invalid host value"))
}
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(MAX_SAFE_INTEGER as u128) as u64
}
impl HostServices {
    pub(crate) fn new(profile: PathBuf, boot_id: String) -> Self {
        Self {
            state: Mutex::new(ServicesState {
                leases: BTreeMap::new(),
                context: HostContext::default(),
                feedback: FeedbackHost::new(boot_id),
                last_ui_revision: 0,
                ui_revision: 0,
                feedback_revision: 0,
                active_project: None,
                source_live: false,
                last_ui_emit: 0,
            }),
            storage: Storage::new(profile),
        }
    }
    pub(crate) fn begin(&self, manifest: &Manifest, generation: &str) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| busy())?;
        if state.leases.contains_key(&manifest.id) {
            return Err(busy());
        }
        self.storage.register(&manifest.id, generation)?;
        if let Err(e) = state.context.register(manifest, generation) {
            self.storage.revoke(&manifest.id, generation);
            return Err(e);
        }
        if let Err(e) = state.feedback.begin(&manifest.id, generation) {
            state.context.revoke(&manifest.id, generation);
            self.storage.revoke(&manifest.id, generation);
            return Err(e);
        }
        state.leases.insert(
            manifest.id.clone(),
            Lease {
                manifest: manifest.clone(),
                generation: generation.into(),
                published: false,
                inflight: false,
                tokens: 20.0,
                last_refill: now_ms(),
            },
        );
        Ok(())
    }
    pub(crate) fn activate(&self, id: &str, generation: &str) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| busy())?;
        state
            .leases
            .get(id)
            .filter(|l| l.generation == generation)
            .ok_or_else(|| err(PLUGIN_STALE_GENERATION, "host generation is stale"))?;
        let result = state
            .context
            .activate(id, generation)
            .and_then(|()| state.feedback.activate(id, generation))
            .and_then(|()| self.storage.activate(id, generation));
        if let Err(e) = result {
            state.context.revoke(id, generation);
            state.feedback.revoke(id, generation);
            self.storage.revoke(id, generation);
            state.leases.remove(id);
            return Err(e);
        }
        state.leases.get_mut(id).expect("validated lease").published = true;
        Ok(())
    }
    pub(crate) fn revoke(&self, id: &str, generation: &str) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.storage.revoke(id, generation);
        if state
            .leases
            .get(id)
            .is_some_and(|l| l.generation == generation)
        {
            state.leases.remove(id);
            state.context.revoke(id, generation);
            state.feedback.revoke(id, generation);
        }
    }
    fn snapshot_locked(state: &mut ServicesState) -> HostUiSnapshot {
        let now = now_ms();
        if state.context.expire(now) {
            state.feedback.clear();
        }
        let current = state.context.current(now).ok();
        if current.is_none() && state.source_live {
            state.feedback.clear();
        }
        state.source_live = current.is_some();
        let active = current.and_then(|(_, c)| c.active_project.map(|p| p.id));
        let mut snapshot = state.feedback.snapshot_for_project(now, active.as_deref());
        if snapshot.revision != state.feedback_revision || active != state.active_project {
            state.feedback_revision = snapshot.revision;
            state.active_project = active;
            state.ui_revision = state.ui_revision.saturating_add(1).min(MAX_SAFE_INTEGER);
        }
        snapshot.revision = state.ui_revision;
        snapshot
    }
    pub(crate) fn call(
        &self,
        id: &str,
        generation: &str,
        request: &ServiceRequest,
    ) -> Result<Value> {
        let mut state = self.state.try_lock().map_err(|_| busy())?;
        let _ = Self::snapshot_locked(&mut state);
        let manifest = state
            .leases
            .get(id)
            .filter(|l| l.generation == generation)
            .map(|l| l.manifest.clone())
            .ok_or_else(|| err(PLUGIN_STALE_GENERATION, "host generation is stale"))?;
        if known_host_service(&request.service, request.version)
            != Some(HostServiceAvailability::Available)
        {
            return Err(err(PLUGIN_NOT_SUPPORTED, "host capability is unavailable"));
        }
        let input = serde_json::to_vec(&request.input)
            .map_err(|_| err(PLUGIN_INVALID_REQUEST, "invalid host input"))?;
        match (request.service.as_str(), request.method.as_str()) {
            ("se.host.context", "snapshot") => {
                let req = parse_host_context_snapshot_request(&input)?;
                encode(
                    &state
                        .context
                        .snapshot(id, generation, req.scope, now_ms())?,
                )
            }
            ("se.host.storage", method @ ("get" | "set" | "delete")) => {
                let req = match method {
                    "get" => StorageRequest::Get(parse_storage_get_request(&input)?),
                    "set" => StorageRequest::Set(parse_storage_set_request(&input)?),
                    _ => StorageRequest::Delete(parse_storage_delete_request(&input)?),
                };
                if let Some(project) = request.input.get("projectId").and_then(Value::as_str) {
                    let (_, context) = state.context.current(now_ms())?;
                    if !context.project_ids.iter().any(|p| p == project) {
                        return Err(err(
                            PLUGIN_INVALID_REQUEST,
                            "storage project is not published",
                        ));
                    }
                }
                encode(&self.storage.accept(id, generation, req)?)
            }
            ("se.host.feedback" | "se.host.status", _) => {
                let active = state
                    .context
                    .current(now_ms())
                    .ok()
                    .and_then(|(_, c)| c.active_project.map(|p| p.id));
                state.feedback.apply(
                    &manifest,
                    generation,
                    active.as_deref(),
                    &request.service,
                    &request.method,
                    &request.input,
                    now_ms(),
                )
            }
            _ => Err(err(PLUGIN_INVALID_REQUEST, "host method is not callable")),
        }
    }
    pub(crate) fn publish(
        &self,
        owner: &str,
        request: HostContextPublishRequest,
    ) -> Result<HostContextAck> {
        let mut state = self.state.lock().map_err(|_| busy())?;
        let result = state.context.publish(owner, request, now_ms());
        let _ = Self::snapshot_locked(&mut state);
        result
    }
    pub(crate) fn heartbeat(&self, owner: &str, source: &str) -> Result<HostContextAck> {
        let mut state = self.state.lock().map_err(|_| busy())?;
        let result = state.context.heartbeat(owner, source, now_ms());
        let _ = Self::snapshot_locked(&mut state);
        result
    }
    pub(crate) fn detach(&self, owner: &str) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.context.detach(owner) {
            state.feedback.clear();
        }
        let _ = Self::snapshot_locked(&mut state);
    }
    pub(crate) fn ui_snapshot(&self) -> Result<HostUiSnapshot> {
        let mut state = self.state.lock().map_err(|_| busy())?;
        Ok(Self::snapshot_locked(&mut state))
    }
    pub(crate) fn dismiss(&self, req: HostUiDismissRequest) -> Result<HostUiDismissResult> {
        let mut state = self.state.lock().map_err(|_| busy())?;
        if !state
            .leases
            .get(&req.plugin_id)
            .is_some_and(|l| l.published && l.generation == req.generation)
        {
            return Err(err(PLUGIN_STALE_GENERATION, "host UI generation is stale"));
        }
        state.feedback.dismiss(&req, now_ms())
    }
    pub(crate) fn resolve_action(
        &self,
        req: &ExecuteContributionRequest,
    ) -> Result<(String, Value)> {
        validate_execute_contribution_request(req)?;
        let mut state = self.state.lock().map_err(|_| busy())?;
        let manifest = state
            .leases
            .get(&req.plugin_id)
            .filter(|l| l.published && l.generation == req.generation)
            .map(|l| l.manifest.clone())
            .ok_or_else(|| {
                err(
                    PLUGIN_STALE_GENERATION,
                    "host contribution generation is stale",
                )
            })?;
        let (revision, context) = state.context.current(now_ms())?;
        let invalid = || {
            err(
                PLUGIN_INVALID_ACTION,
                "host contribution target is unavailable",
            )
        };
        if req.context_revision > revision {
            return Err(invalid());
        }
        let mut target = req.target.clone();
        if matches!(
            req.kind,
            ContributionKind::Status
                | ContributionKind::Notification
                | ContributionKind::Progress
                | ContributionKind::Keybinding
        ) {
            let stored_project = if req.kind == ContributionKind::Status {
                state
                    .feedback
                    .snapshot(now_ms())
                    .status_items
                    .iter()
                    .find(|item| {
                        item.plugin_id == req.plugin_id
                            && item.generation == req.generation
                            && item.id == req.contribution_id
                    })
                    .and_then(|item| item.project_id.clone())
            } else {
                None
            };
            let project =
                stored_project.or_else(|| context.active_project.as_ref().map(|p| p.id.clone()));
            if target
                .project_id
                .as_ref()
                .is_some_and(|p| Some(p) != project.as_ref())
                || target
                    .terminal_id
                    .as_ref()
                    .is_some_and(|t| Some(t) != context.active_terminal_id.as_ref())
                || target
                    .session_id
                    .as_ref()
                    .is_some_and(|s| Some(s) != context.active_session_id.as_ref())
            {
                return Err(invalid());
            }
            target.project_id = project;
            target.terminal_id = context.active_terminal_id.clone();
            target.session_id = context.active_session_id.clone();
            if req.kind == ContributionKind::Keybinding
                && (context.focus.text_input || context.focus.terminal)
            {
                return Err(invalid());
            }
        }
        if target
            .project_id
            .as_ref()
            .is_some_and(|id| !context.project_ids.contains(id))
        {
            return Err(invalid());
        }
        if let Some(id) = &target.terminal_id {
            let terminal = context
                .terminals
                .iter()
                .find(|t| &t.id == id)
                .ok_or_else(invalid)?;
            if target.project_id.is_some() && target.project_id != terminal.project_id {
                return Err(invalid());
            }
            target.project_id = terminal.project_id.clone();
        }
        if let Some(id) = &target.session_id {
            let session = context
                .agent_sessions
                .iter()
                .find(|s| &s.id == id)
                .ok_or_else(invalid)?;
            if target
                .project_id
                .as_ref()
                .is_some_and(|p| p != &session.project_id)
            {
                return Err(invalid());
            }
            target.project_id = Some(session.project_id.clone());
        }
        let mut when = None;
        let (command, source, data) = match req.kind {
            ContributionKind::Menu => {
                let menu = manifest
                    .contributes
                    .menus
                    .iter()
                    .find(|m| m.id == req.contribution_id)
                    .ok_or_else(invalid)?;
                let source = match menu.location {
                    MenuLocation::TerminalTab => {
                        if target.terminal_id.is_none() {
                            return Err(invalid());
                        }
                        "terminal.tab"
                    }
                    MenuLocation::TerminalContext => {
                        if target.terminal_id.is_none() {
                            return Err(invalid());
                        }
                        "terminal.context"
                    }
                    MenuLocation::ProjectContext => {
                        if target.project_id.is_none() {
                            return Err(invalid());
                        }
                        "project.context"
                    }
                    MenuLocation::AgentSessionContext => {
                        if target.session_id.is_none() {
                            return Err(invalid());
                        }
                        "agentSession.context"
                    }
                };
                when = menu.when.as_ref();
                (menu.command_id.clone(), source, Value::Null)
            }
            ContributionKind::Keybinding => {
                let binding = manifest
                    .contributes
                    .keybindings
                    .iter()
                    .find(|b| b.id == req.contribution_id)
                    .ok_or_else(invalid)?;
                when = binding.when.as_ref();
                (binding.command_id.clone(), "keybinding", Value::Null)
            }
            ContributionKind::Status
            | ContributionKind::Notification
            | ContributionKind::Progress => {
                let active = context.active_project.as_ref().map(|p| p.id.as_str());
                let action = state.feedback.resolve_action(
                    &manifest,
                    &req.generation,
                    req,
                    active,
                    now_ms(),
                )?;
                let source = match req.kind {
                    ContributionKind::Status => "statusBar",
                    ContributionKind::Notification => "notification",
                    _ => "progress",
                };
                (action.command_id, source, action.args)
            }
        };
        if let Some(when) = when {
            let conditions = [
                (when.project_exists, target.project_id.is_some()),
                (when.terminal_exists, target.terminal_id.is_some()),
                (when.agent_session_exists, target.session_id.is_some()),
                (when.text_input_focused, context.focus.text_input),
                (when.terminal_focused, context.focus.terminal),
            ];
            if conditions
                .iter()
                .any(|(required, actual)| required.is_some_and(|r| r != *actual))
            {
                return Err(invalid());
            }
        }
        if !manifest
            .contributes
            .commands
            .iter()
            .any(|c| c.id == command)
        {
            return Err(invalid());
        }
        Ok((
            command,
            json!({"source":source,"context":{"projectId":target.project_id,"terminalId":target.terminal_id,
            "sessionId":target.session_id,"contextRevision":revision},"data":data}),
        ))
    }
    pub(crate) fn next_delivery(&self, id: &str, generation: &str) -> Option<Value> {
        let mut state = self.state.lock().ok()?;
        let now = now_ms();
        let lease = state.leases.get_mut(id)?;
        if lease.generation != generation || !lease.published || lease.inflight {
            return None;
        }
        lease.tokens =
            (lease.tokens + now.saturating_sub(lease.last_refill) as f64 * 0.020).min(20.0);
        lease.last_refill = now;
        if lease.tokens < 1.0 {
            return None;
        }
        let delivery = if let Some(completion) = self.storage.pop_completion_for(id, generation) {
            json!({"schemaVersion":1,"service":"se.host.storage","version":1,"method":"completed","input":completion.input})
        } else {
            let event = state.context.pop(id, generation, now)?;
            json!({"schemaVersion":1,"service":"se.host.events","version":1,"method":"deliver","input":event})
        };
        let lease = state.leases.get_mut(id)?;
        lease.inflight = true;
        lease.tokens -= 1.0;
        Some(delivery)
    }
    pub(crate) fn delivery_done(&self, id: &str, generation: &str, delivery: Value, retry: bool) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(lease) = state
            .leases
            .get_mut(id)
            .filter(|l| l.generation == generation)
        else {
            return;
        };
        lease.inflight = false;
        if delivery["service"] == "se.host.storage" {
            if let Ok(input) =
                serde_json::from_value::<StorageCompletionInput>(delivery["input"].clone())
            {
                if retry {
                    self.storage
                        .return_completion(crate::storage::StorageCompletion {
                            plugin_id: id.into(),
                            generation: generation.into(),
                            input,
                        });
                } else {
                    self.storage
                        .finish_completion(id, generation, &input.ticket_id);
                }
            }
        } else if retry {
            if let Ok(event) =
                serde_json::from_value::<HostEventDeliveryInput>(delivery["input"].clone())
            {
                state.context.put_back(id, generation, event);
            }
        }
    }
    pub(crate) fn take_ui_changed(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let now = now_ms();
        let snapshot = Self::snapshot_locked(&mut state);
        if snapshot.revision == state.last_ui_revision
            || now.saturating_sub(state.last_ui_emit) < 100
        {
            return false;
        }
        state.last_ui_revision = snapshot.revision;
        state.last_ui_emit = now;
        true
    }
    pub(crate) fn context_counts(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        (
            state.context.resource_count(),
            state.context.subscription_count(),
        )
    }
    pub(crate) fn ui_count(&self) -> usize {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let snapshot = state.feedback.snapshot(now_ms());
        snapshot.notifications.len() + snapshot.progress.len() + snapshot.status_items.len()
    }
    pub(crate) fn storage_count(&self) -> usize {
        self.storage.pending_count()
    }
    pub(crate) fn counts(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        (
            state.leases.len(),
            state.leases.values().filter(|l| l.inflight).count(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const GENERATION: &str = "00000000-0000-4000-8000-000000000001";
    fn manifest() -> Manifest {
        serde_json::from_value(json!({"schemaVersion":1,"id":"test.host","name":"Host","version":"1.0.0",
            "abi":{"min":1,"max":1},
            "services":{"provides":[],"requires":[{"name":"se.host.context","version":1},{"name":"se.host.events","version":1},
                {"name":"se.host.storage","version":1},{"name":"se.host.feedback","version":1}]},
            "contributes":{"commands":[{"id":"echo","title":"Echo"}],"configuration":{"title":"Settings","properties":{}},"views":[],
                "eventSubscriptions":[{"id":"project","topic":"project.activeChanged","scope":"window"}],
                "menus":[{"id":"terminal","location":"terminal.context","commandId":"echo","when":{"terminal.exists":true}}]}
        })).unwrap()
    }
    fn publish(host: &HostServices) {
        let request = serde_json::from_value(json!({"sourceId":"00000000-0000-4000-8000-000000000010","sourceSequence":1,"reset":true,
            "snapshot":{"activeProject":{"id":"p1","name":"Project","path":"/project"},"projectIds":["p1","p2"],
                "activeTerminalId":"t1","activeSessionId":null,"terminals":[{"id":"t1","projectId":"p1","ptyId":null},
                {"id":"t2","projectId":"p2","ptyId":null}],"agentSessions":[],"focus":{"textInput":false,"terminal":false},"truncated":false},"events":[]})).unwrap();
        host.publish("owner", request).unwrap();
    }
    fn request(service: &str, method: &str, input: Value) -> ServiceRequest {
        ServiceRequest {
            schema_version: 1,
            service: service.into(),
            version: 1,
            method: method.into(),
            input,
        }
    }
    #[test]
    fn withdraw_revokes_pending_host_resources() {
        let profile = tempfile::tempdir().unwrap();
        let host = HostServices::new(profile.path().into(), GENERATION.into());
        let manifest = manifest();
        host.begin(&manifest, GENERATION).unwrap();
        publish(&host);
        host.call(
            &manifest.id,
            GENERATION,
            &request(
                "se.host.feedback",
                "notification.upsert",
                json!({"id":"notice","severity":"info","title":"Hello"}),
            ),
        )
        .unwrap();
        host.call(&manifest.id,GENERATION,&request("se.host.storage","set",json!({"requestId":"save","scope":"profile","key":"value","value":null,"expectedRevision":0}))).unwrap();
        assert!(host.ui_snapshot().unwrap().notifications.is_empty());
        assert_eq!(host.storage.pending_count(), 1);
        host.revoke(&manifest.id, GENERATION);
        assert_eq!(host.counts(), (0, 0));
        assert_eq!(host.storage.pending_count(), 0);
        assert!(host.ui_snapshot().unwrap().notifications.is_empty());
        assert!(!profile.path().join("plugins/data/test.host.json").exists());
        host.begin(&manifest, GENERATION).unwrap();
        host.activate(&manifest.id, GENERATION).unwrap();
        host.call(
            &manifest.id,
            GENERATION,
            &request(
                "se.host.feedback",
                "notification.upsert",
                json!({"id":"notice","severity":"info","title":"Hello"}),
            ),
        )
        .unwrap();
        assert_eq!(host.ui_snapshot().unwrap().notifications.len(), 1);
        assert!(host.next_delivery(&manifest.id, GENERATION).is_some());
        host.revoke(&manifest.id, GENERATION);
        assert_eq!(host.counts(), (0, 0));
        assert!(host.ui_snapshot().unwrap().notifications.is_empty());
    }
    #[test]
    fn host_actions_reject_stale_generation_and_targets() {
        let profile = tempfile::tempdir().unwrap();
        let host = HostServices::new(profile.path().into(), GENERATION.into());
        let manifest = manifest();
        host.begin(&manifest, GENERATION).unwrap();
        host.activate(&manifest.id, GENERATION).unwrap();
        publish(&host);
        let mut req: ExecuteContributionRequest = serde_json::from_value(json!({"pluginId":"test.host","generation":GENERATION,
            "kind":"menu","contributionId":"terminal","contextRevision":1,"target":{"projectId":"p1","terminalId":"t1"}})).unwrap();
        let (command, args) = host.resolve_action(&req).unwrap();
        assert_eq!(command, "echo");
        assert_eq!(args["source"], "terminal.context");
        assert_eq!(args["context"]["terminalId"], "t1");
        req.generation = "00000000-0000-4000-8000-000000000099".into();
        assert_eq!(
            host.resolve_action(&req).unwrap_err().code,
            PLUGIN_STALE_GENERATION
        );
        req.generation = GENERATION.into();
        req.target.terminal_id = Some("missing".into());
        assert_eq!(
            host.resolve_action(&req).unwrap_err().code,
            PLUGIN_INVALID_ACTION
        );
        req.target.terminal_id = Some("t2".into());
        assert_eq!(
            host.resolve_action(&req).unwrap_err().code,
            PLUGIN_INVALID_ACTION
        );
        req.target.terminal_id = Some("t1".into());
        req.context_revision = 2;
        assert_eq!(
            host.resolve_action(&req).unwrap_err().code,
            PLUGIN_INVALID_ACTION
        );
        host.call(&manifest.id,GENERATION,&request("se.host.feedback","notification.upsert",json!({"id":"notice","severity":"info","title":"Hello","actions":[{"label":"Echo","commandId":"echo","args":{"kept":true}}]}))).unwrap();
        req.kind = ContributionKind::Notification;
        req.contribution_id = "notice".into();
        req.action_index = Some(0);
        req.context_revision = 1;
        req.target = ContributionTarget {
            project_id: None,
            terminal_id: None,
            session_id: None,
        };
        let (_, args) = host.resolve_action(&req).unwrap();
        assert_eq!(args["context"]["terminalId"], "t1");
        assert_eq!(args["data"]["kept"], true);
        req.target.project_id = Some("p2".into());
        assert_eq!(
            host.resolve_action(&req).unwrap_err().code,
            PLUGIN_INVALID_ACTION
        );
        host.detach("wrong-owner");
        assert_eq!(host.ui_snapshot().unwrap().notifications.len(), 1);
        host.detach("owner");
        assert!(host.ui_snapshot().unwrap().notifications.is_empty());
    }
}
