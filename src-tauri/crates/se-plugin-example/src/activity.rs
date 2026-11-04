//! A real ABI-v1 host-capability consumer. Disk operations are ticketed: no
//! callback waits for a completion, and a conflicting CAS is read/recomputed.
use se_plugin_abi::{Host, SeStatus, SEP_INTERNAL, SEP_UNAVAILABLE};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
struct Pending {
    kind: &'static str,
    project: Option<String>,
    ticket: String,
    amount: u64,
    enabled: bool,
    base_revision: u64,
    target_value: u64,
}

#[derive(Default)]
struct ProjectState {
    loaded: bool,
    revision: u64,
    observed: u64,
    unsaved: u64,
    uncertain: Option<Pending>,
    waiting_turns: u64,
}

pub(crate) struct Activity {
    source: String,
    sequences: BTreeMap<String, u64>,
    revision: u64,
    context: Value,
    profile_loaded: bool,
    project_loaded: bool,
    enabled: bool,
    desired_enabled: Option<bool>,
    profile_revision: u64,
    project_revision: u64,
    observed: u64,
    unsaved: u64,
    uncertain: Option<Pending>,
    waiting_turns: u64,
    // One idempotent attempt per outstanding effect on each callback. Never
    // discard failed cleanup: its constant-size debt survives until acknowledged.
    cleanup: [bool; 4],
    projects: BTreeMap<String, ProjectState>,
    nonce: u64,
    pending: BTreeMap<String, Pending>,
    presentation: Option<bool>,
    last_target: Value,
    #[cfg(feature = "test-hooks")]
    fault: String,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            source: String::new(),
            sequences: BTreeMap::new(),
            revision: 0,
            context: Value::Null,
            profile_loaded: false,
            project_loaded: false,
            enabled: true,
            desired_enabled: None,
            profile_revision: 0,
            project_revision: 0,
            observed: 0,
            unsaved: 0,
            uncertain: None,
            waiting_turns: 0,
            cleanup: [false; 4],
            projects: BTreeMap::new(),
            nonce: 0,
            pending: BTreeMap::new(),
            presentation: None,
            last_target: Value::Null,
            #[cfg(feature = "test-hooks")]
            fault: String::new(),
        }
    }
}

fn call(host: &Host, service: &str, method: &str, input: Value) -> Result<Value, SeStatus> {
    let bytes = serde_json::to_vec(
        &json!({"schemaVersion":1,"service":service,"version":1,"method":method,"input":input}),
    )
    .map_err(|_| SEP_INTERNAL)?;
    let mut output = vec![0; 256 * 1024];
    let written = host.broker_call(&bytes, &mut output)?;
    let reply: Value = serde_json::from_slice(&output[..written]).map_err(|_| SEP_INTERNAL)?;
    if reply["result"]["ok"] == true {
        Ok(reply["result"]["value"].clone())
    } else {
        Err(SEP_UNAVAILABLE)
    }
}

fn ended(previous: &Value, next: &Value, removed: bool) -> bool {
    !removed
        && previous["activeTurn"] == true
        && previous["replaying"] == false
        && next["activeTurn"] == false
        && next["replaying"] == false
        && next["status"] == "active"
}

// A namespace revision alone is insufficient: another key may have changed.
// Require both publication evidence and a value that includes this exact delta.
fn reconcile(uncertain: &mut Option<Pending>, revision: u64, value: u64, unsaved: &mut u64) {
    if let Some(write) = uncertain.take() {
        if revision > write.base_revision && value >= write.target_value {
            *unsaved = unsaved.saturating_sub(write.amount);
        }
    }
}

impl Activity {
    fn retry_cleanup(&mut self, host: &Host) {
        let operations = [
            (
                "se.host.feedback",
                "progress.upsert",
                json!({"id":"activity-turn","title":"Activity disabled","state":"cancelled"}),
            ),
            (
                "se.host.feedback",
                "notification.close",
                json!({"id":"activity-summary"}),
            ),
            (
                "se.host.feedback",
                "notification.close",
                json!({"id":"activity-turn-ended"}),
            ),
            ("se.host.status", "clear", json!({"id":"activity"})),
        ];
        // At most four attempts per callback, with terminal progress first.
        // No spin/sleep, and no failed effect is forgotten after a retry cap.
        // Teardown needs no broker calls: host generation revocation removes UI.
        for (pending, (service, method, input)) in self.cleanup.iter_mut().zip(operations) {
            if *pending && call(host, service, method, input).is_ok() {
                *pending = false;
            }
        }
    }
    fn apply_waiting_turns(&mut self, host: &Host) {
        if !self.ready() || self.waiting_turns == 0 {
            return;
        }
        let amount = std::mem::take(&mut self.waiting_turns);
        if self.enabled {
            self.unsaved = self.unsaved.saturating_add(amount);
            let _ = call(
                host,
                "se.host.feedback",
                "notification.upsert",
                json!({"id":"activity-turn-ended","severity":"info","title":"Agent turn ended","message":"An observed agent turn ended; this is not a success assertion.","actions":[{"label":"Summary","commandId":"example.activity-summary","args":null}]}),
            );
        }
    }
    fn project(&self) -> Option<String> {
        self.context["activeProject"]["id"]
            .as_str()
            .map(str::to_owned)
    }
    fn ready(&self) -> bool {
        self.profile_loaded && self.project_loaded
    }
    fn active(&self) -> bool {
        self.context["agentSessions"]
            .as_array()
            .is_some_and(|sessions| {
                sessions.iter().any(|s| {
                    s["activeTurn"] == true && s["replaying"] == false && s["status"] == "active"
                })
            })
    }
    pub(crate) fn state(&self) -> &str {
        if !self.ready() {
            "Loading activity"
        } else if !self.enabled {
            "Activity disabled"
        } else if self.active() {
            "Agent active"
        } else {
            "Agent idle"
        }
    }
    pub(crate) fn terminal_count(&self) -> usize {
        self.context["terminals"].as_array().map_or(0, Vec::len)
    }
    pub(crate) fn observed(&self) -> u64 {
        self.observed.saturating_add(self.unsaved)
    }
    fn has(&self, kind: &str) -> bool {
        self.pending
            .values()
            .any(|p| p.kind == kind && p.project == self.project_for(kind))
    }
    fn project_for(&self, kind: &str) -> Option<String> {
        if kind.starts_with("project") {
            self.project()
        } else {
            None
        }
    }
    fn storage(&mut self, host: &Host, kind: &'static str, amount: u64) {
        if self.has(kind) {
            return;
        }
        let project = self.project_for(kind);
        if kind.starts_with("project") && project.is_none() {
            return;
        }
        self.nonce += 1;
        let id = format!("activity-{}", self.nonce);
        let profile = kind.starts_with("profile");
        let mut input = json!({"requestId":id,"scope":if profile {"profile"} else {"project"},"key":if profile {"activityEnabled"} else {"observedTurns"}});
        if let Some(project) = &project {
            input["projectId"] = json!(project);
        }
        let set = kind.ends_with("set");
        if set {
            input["expectedRevision"] = json!(if profile {
                self.profile_revision
            } else {
                self.project_revision
            });
            input["value"] = if profile {
                json!(self.enabled)
            } else {
                json!(self.observed.saturating_add(amount))
            };
        }
        if let Ok(ticket) = call(
            host,
            "se.host.storage",
            if set { "set" } else { "get" },
            input,
        ) {
            self.pending.insert(
                id,
                Pending {
                    kind,
                    project,
                    ticket: ticket["ticketId"].as_str().unwrap_or_default().into(),
                    amount,
                    enabled: self.enabled,
                    base_revision: self.project_revision,
                    target_value: self.observed.saturating_add(amount),
                },
            );
        }
    }
    pub(crate) fn initialize_web(&mut self, host: &Host) {
        if self.source.is_empty() {
            self.snapshot(host);
        } else {
            self.initialize(host);
        }
    }
    fn initialize(&mut self, host: &Host) {
        if !self.profile_loaded {
            self.storage(host, "profile-get", 0);
        }
        if !self.project_loaded {
            if self.project().is_some() {
                self.storage(host, "project-get", 0);
            } else {
                self.project_loaded = true;
            }
        }
    }
    fn flush(&mut self, host: &Host) {
        self.apply_waiting_turns(host);
        if self.profile_loaded && self.desired_enabled.is_some() && !self.has("profile-set") {
            self.storage(host, "profile-set", 0);
        }
        if self.project_loaded && self.unsaved > 0 && !self.has("project-set") {
            self.storage(host, "project-set", self.unsaved);
        }
    }
    fn present(&mut self, host: &Host) {
        if self.cleanup.iter().any(|pending| *pending)
            || !self.ready()
            || !self.enabled
            || self.project().is_none()
        {
            return;
        }
        let active = self.active();
        if self.presentation == Some(active) {
            return;
        }
        let status = call(
            host,
            "se.host.status",
            "set",
            json!({"id":"activity","text":if active {"Agent active"} else {"Agent idle"},"projectId":self.project(),"icon":"calculator"}),
        );
        let progress = call(
            host,
            "se.host.feedback",
            "progress.upsert",
            json!({"id":"activity-turn","title":"Agent activity","state":if active {"running"} else {"completed"}}),
        );
        if status.is_ok() && progress.is_ok() {
            self.presentation = Some(active);
        }
    }
    fn switch_project(&mut self, old_project: Option<String>, new_project: Option<String>) {
        if old_project == new_project {
            return;
        }
        if let Some(old) = old_project {
            self.projects.insert(
                old,
                ProjectState {
                    loaded: self.project_loaded,
                    revision: self.project_revision,
                    observed: self.observed,
                    unsaved: self.unsaved,
                    uncertain: self.uncertain.take(),
                    waiting_turns: std::mem::take(&mut self.waiting_turns),
                },
            );
        }
        let state = new_project
            .and_then(|id| self.projects.remove(&id))
            .unwrap_or_default();
        self.project_loaded = state.loaded;
        self.project_revision = state.revision;
        self.observed = state.observed;
        self.unsaved = state.unsaved;
        self.uncertain = state.uncertain;
        self.waiting_turns = state.waiting_turns;
    }
    fn snapshot(&mut self, host: &Host) {
        let Ok(snapshot) = call(
            host,
            "se.host.context",
            "snapshot",
            json!({"scope":"activeProject"}),
        ) else {
            return;
        };
        let old_project = self.project();
        let changed = self.source != snapshot["sourceId"].as_str().unwrap_or_default()
            || self.revision != snapshot["contextRevision"].as_u64().unwrap_or_default();
        self.source = snapshot["sourceId"].as_str().unwrap_or_default().into();
        self.revision = snapshot["contextRevision"].as_u64().unwrap_or_default();
        self.sequences = serde_json::from_value(snapshot["sequences"].clone()).unwrap_or_default();
        self.context = snapshot["context"].clone();
        self.switch_project(old_project, self.project());
        if changed {
            self.presentation = None;
        }
        self.initialize(host);
        self.flush(host);
        self.present(host);
    }
    pub(crate) fn deliver(&mut self, host: &Host, input: &Value) {
        self.retry_cleanup(host);
        #[cfg(feature = "test-hooks")]
        match self.fault.as_str() {
            "panic" => panic!("intentional activity delivery panic"),
            "timeout" => std::thread::sleep(std::time::Duration::from_secs(3)),
            _ => (),
        }
        let subscription = input["subscriptionId"].as_str().unwrap_or_default();
        let sequence = input["sequence"].as_u64().unwrap_or_default();
        let previous_sequence = self
            .sequences
            .get(subscription)
            .copied()
            .unwrap_or_default();
        let same_source = self.source == input["sourceId"].as_str().unwrap_or_default();
        if same_source && sequence <= previous_sequence {
            return;
        }
        if !same_source
            || input["resyncRequired"] == true
            || sequence != previous_sequence + 1
            || input["topic"] == "project.activeChanged"
        {
            self.snapshot(host);
            return;
        }
        self.sequences.insert(subscription.into(), sequence);
        self.revision = input["contextRevision"].as_u64().unwrap_or(self.revision);
        let data = &input["data"];
        match input["topic"].as_str() {
            Some("terminal.created") => {
                if let Some(terminals) = self.context["terminals"].as_array_mut() {
                    if !terminals.iter().any(|t| t["id"] == data["terminal"]["id"]) {
                        terminals.push(data["terminal"].clone());
                    }
                }
            }
            Some("terminal.closed") => {
                if let Some(terminals) = self.context["terminals"].as_array_mut() {
                    terminals.retain(|t| t["id"] != data["terminal"]["id"]);
                }
                if self.context["activeTerminalId"] == data["terminal"]["id"] {
                    self.context["activeTerminalId"] = Value::Null;
                }
            }
            Some("terminal.activeChanged") => {
                self.context["activeTerminalId"] = data["terminalId"].clone()
            }
            Some("agent.session.stateChanged") => {
                let next = &data["session"];
                if let Some(sessions) = self.context["agentSessions"].as_array_mut() {
                    let previous = sessions
                        .iter()
                        .find(|s| s["id"] == next["id"])
                        .cloned()
                        .unwrap_or(Value::Null);
                    let turn_ended = ended(&previous, next, data["removed"] == true);
                    sessions.retain(|s| s["id"] != next["id"]);
                    if data["removed"] != true {
                        sessions.push(next.clone());
                    }
                    if turn_ended && (!self.profile_loaded || self.enabled) {
                        self.waiting_turns = self.waiting_turns.saturating_add(1);
                    }
                }
            }
            _ => (),
        }
        self.initialize(host);
        self.flush(host);
        self.present(host);
    }
    pub(crate) fn completed(&mut self, host: &Host, input: &Value) {
        self.retry_cleanup(host);
        let id = input["requestId"].as_str().unwrap_or_default();
        let Some(pending) = self.pending.get(id) else {
            return;
        };
        if input["ticketId"] != pending.ticket {
            return;
        }
        let pending = self.pending.remove(id).expect("checked ticket");
        if pending.project != self.project_for(pending.kind) {
            if let Some(project) = &pending.project {
                let state = self.projects.entry(project.clone()).or_default();
                let result = &input["result"];
                if result["ok"] == true {
                    state.revision = result["value"]["revision"].as_u64().unwrap_or_default();
                    state.loaded = true;
                    if pending.kind == "project-get" {
                        state.observed = result["value"]["value"].as_u64().unwrap_or_default();
                        reconcile(
                            &mut state.uncertain,
                            state.revision,
                            state.observed,
                            &mut state.unsaved,
                        );
                    } else if pending.kind == "project-set" {
                        state.observed = state.observed.saturating_add(pending.amount);
                        state.unsaved = state.unsaved.saturating_sub(pending.amount);
                    }
                } else {
                    // Retry from a fresh read only when this project is active.
                    // Keep the unsaved delta; never apply it to another project.
                    state.loaded = false;
                    if pending.kind == "project-set"
                        && result["error"]["code"] != "PLUGIN_STORAGE_CONFLICT"
                    {
                        state.uncertain = Some(pending.clone());
                    }
                }
            }
            return;
        }
        let result = &input["result"];
        let profile = pending.kind.starts_with("profile");
        if result["ok"] != true {
            // Conflict/uncertain completion: read before recomputing. Never
            // replay a stale mutation or assume it failed before publication.
            if profile {
                self.profile_loaded = false;
            } else {
                self.project_loaded = false;
                if pending.kind == "project-set"
                    && result["error"]["code"] != "PLUGIN_STORAGE_CONFLICT"
                {
                    self.uncertain = Some(pending.clone());
                }
            }
            // A failed read stays loading until a later event/summary retries;
            // never create an unbounded completion -> read-error loop.
            if pending.kind.ends_with("set") {
                self.storage(
                    host,
                    if profile {
                        "profile-get"
                    } else {
                        "project-get"
                    },
                    0,
                );
            }
            return;
        }
        let value = &result["value"];
        let revision = value["revision"].as_u64().unwrap_or_default();
        match pending.kind {
            "profile-get" => {
                self.profile_revision = revision;
                self.profile_loaded = true;
                self.enabled = self
                    .desired_enabled
                    .unwrap_or_else(|| value["value"].as_bool().unwrap_or(true));
            }
            "project-get" => {
                self.project_revision = revision;
                self.project_loaded = true;
                self.observed = value["value"].as_u64().unwrap_or_default();
                reconcile(
                    &mut self.uncertain,
                    revision,
                    self.observed,
                    &mut self.unsaved,
                );
            }
            "profile-set" => {
                self.profile_revision = revision;
                if self.desired_enabled == Some(pending.enabled) {
                    self.desired_enabled = None;
                }
            }
            "project-set" => {
                self.project_revision = revision;
                self.observed = self.observed.saturating_add(pending.amount);
                self.unsaved = self.unsaved.saturating_sub(pending.amount);
            }
            _ => (),
        }
        self.flush(host);
        self.present(host);
    }
    pub(crate) fn command(
        &mut self,
        host: &Host,
        command: &str,
        args: &Value,
    ) -> Result<Value, SeStatus> {
        self.retry_cleanup(host);
        match command {
            "example.activity-summary" => {
                self.last_target = args.clone();
                self.initialize(host);
                self.flush(host);
                self.present(host);
                let message = format!(
                    "{}; {} terminals; {} observed turns",
                    self.state(),
                    self.terminal_count(),
                    self.observed()
                );
                let _ = call(
                    host,
                    "se.host.feedback",
                    "notification.upsert",
                    json!({"id":"activity-summary","severity":"info","title":"Activity summary","message":message}),
                );
            }
            "example.activity-toggle" => {
                self.enabled = !self.enabled;
                self.desired_enabled = Some(self.enabled);
                self.presentation = None;
                if !self.enabled {
                    self.waiting_turns = 0;
                    self.cleanup = [true; 4];
                    self.retry_cleanup(host);
                }
                self.flush(host);
                self.present(host);
            }
            #[cfg(feature = "test-hooks")]
            "example.activity-inspect" => {
                // Force a stale namespace revision, not a disk edit, to prove
                // conflict completion -> get -> recomputed CAS through the ABI.
                if args["forceConflict"] == true && self.project_revision > 0 {
                    self.project_revision -= 1;
                    self.unsaved += 1;
                    self.flush(host);
                }
            }
            #[cfg(feature = "test-hooks")]
            "example.activity-delivery-fault" => {
                self.fault = args["mode"].as_str().unwrap_or_default().into();
            }
            _ => return Err(SEP_UNAVAILABLE),
        }
        Ok(self.inspect())
    }
    fn inspect(&self) -> Value {
        json!({"sourceId":self.source,"sequences":self.sequences,"contextRevision":self.revision,"pendingTicketIds":self.pending.values().map(|p| &p.ticket).collect::<Vec<_>>(),"observedTarget":self.last_target,"state":self.state(),"enabled":self.enabled,"observedTurns":self.observed(),"terminalCount":self.terminal_count(),"activeTerminalId":self.context["activeTerminalId"],"profileRevision":self.profile_revision,"projectRevision":self.project_revision})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_contiguous_nonreplayed_active_turn_end_is_counted() {
        let start =
            json!({"activeTurn":true,"replaying":false,"status":"active","openTurnId":"turn-1"});
        let idle =
            json!({"activeTurn":false,"replaying":false,"status":"active","openTurnId":null});
        assert!(ended(&start, &idle, false));
        assert!(!ended(&start, &idle, true));
        for status in ["closed", "error", "initializing"] {
            let mut next = idle.clone();
            next["status"] = json!(status);
            assert!(!ended(&start, &next, false));
        }
        let mut replay = start.clone();
        replay["replaying"] = json!(true);
        assert!(!ended(&replay, &idle, false));
        let mut replay = idle.clone();
        replay["replaying"] = json!(true);
        assert!(!ended(&start, &replay, false));
        assert!(!ended(&Value::Null, &idle, false));
    }
    fn unavailable_host() -> Host {
        use se_plugin_abi::{SeHostApiV1, SeOutput, SeSlice, SEP_ABI_VERSION};
        unsafe extern "C" fn log(_: *mut core::ffi::c_void, _: u32, _: SeSlice) -> SeStatus {
            SEP_UNAVAILABLE
        }
        unsafe extern "C" fn broker(
            _: *mut core::ffi::c_void,
            _: SeSlice,
            _: *mut SeOutput,
        ) -> SeStatus {
            panic!("stale completions must not issue another broker operation");
        }
        let api = SeHostApiV1 {
            abi_version: SEP_ABI_VERSION,
            struct_size: core::mem::size_of::<SeHostApiV1>() as u32,
            host_context: core::ptr::null_mut(),
            log: Some(log),
            broker_call: Some(broker),
        };
        // SAFETY: from_raw copies the table; these static callbacks do not
        // dereference the null context and outlive the returned test handle.
        unsafe { Host::from_raw(&api) }.unwrap()
    }

    #[test]
    fn failed_read_waits_for_a_new_observation_instead_of_retrying_in_a_loop() {
        let host = unavailable_host();
        let mut activity = Activity::default();
        activity.pending.insert(
            "activity-1".into(),
            Pending {
                kind: "profile-get",
                project: None,
                ticket: "read-ticket".into(),
                amount: 0,
                enabled: true,
                ..Pending::default()
            },
        );
        activity.completed(&host, &json!({"requestId":"activity-1","ticketId":"read-ticket","result":{"ok":false,"error":{"code":"PLUGIN_STORAGE_IO","message":"unreadable"}}}));
        assert!(!activity.profile_loaded);
        assert!(activity.pending.is_empty());
    }

    #[test]
    fn old_generation_completion_cannot_match_a_reused_request_id() {
        let host = unavailable_host();
        let mut upgraded = Activity::default();
        upgraded.pending.insert(
            "activity-1".into(),
            Pending {
                kind: "profile-get",
                project: None,
                ticket: "new-generation-ticket".into(),
                amount: 0,
                enabled: true,
                ..Pending::default()
            },
        );
        let completion = json!({"requestId":"activity-1","ticketId":"old-generation-ticket","result":{"ok":true,"value":{"found":true,"value":false,"revision":91}}});
        upgraded.completed(&host, &completion);
        assert!(!upgraded.profile_loaded);
        assert!(upgraded.enabled);
        assert_eq!(upgraded.profile_revision, 0);
        assert_eq!(upgraded.pending.len(), 1);
        upgraded.pending.clear();
        upgraded.completed(&host, &completion);
        assert!(!upgraded.profile_loaded);
        assert_eq!(upgraded.profile_revision, 0);
    }

    #[test]
    fn previous_project_completion_does_not_change_new_project_baseline() {
        let host = unavailable_host();
        let mut activity = Activity {
            context: json!({"activeProject":{"id":"p2"}}),
            ..Activity::default()
        };
        activity.pending.insert(
            "activity-2".into(),
            Pending {
                kind: "project-get",
                project: Some("p1".into()),
                ticket: "p1-ticket".into(),
                amount: 0,
                enabled: true,
                ..Pending::default()
            },
        );
        activity.completed(&host, &json!({"requestId":"activity-2","ticketId":"p1-ticket","result":{"ok":true,"value":{"found":true,"value":78,"revision":2}}}));
        assert_eq!(activity.observed(), 0);
        assert!(!activity.project_loaded);
        assert!(activity.pending.is_empty());
    }

    #[test]
    fn project_switch_keeps_inflight_cas_and_extra_unsaved_observations() {
        let host = unavailable_host();
        let mut activity = Activity {
            context: json!({"activeProject":{"id":"p1"}}),
            project_loaded: true,
            project_revision: 4,
            observed: 4,
            unsaved: 2,
            ..Activity::default()
        };
        activity.pending.insert(
            "activity-1".into(),
            Pending {
                kind: "project-set",
                project: Some("p1".into()),
                ticket: "p1-write".into(),
                amount: 1,
                enabled: true,
                ..Pending::default()
            },
        );
        activity.context = json!({"activeProject":{"id":"p2"}});
        activity.switch_project(Some("p1".into()), Some("p2".into()));
        activity.completed(&host, &json!({"requestId":"activity-1","ticketId":"p1-write","result":{"ok":true,"value":{"revision":5}}}));
        assert_eq!(activity.observed(), 0);
        activity.context = json!({"activeProject":{"id":"p1"}});
        activity.switch_project(Some("p2".into()), Some("p1".into()));
        assert_eq!(activity.project_revision, 5);
        assert_eq!(activity.observed, 5);
        assert_eq!(activity.unsaved, 1);
        assert_eq!(activity.observed(), 6);
        activity.pending.insert(
            "activity-2".into(),
            Pending {
                kind: "project-set",
                project: Some("p1".into()),
                ticket: "p1-conflict".into(),
                amount: 1,
                enabled: true,
                ..Pending::default()
            },
        );
        activity.context = json!({"activeProject":{"id":"p2"}});
        activity.switch_project(Some("p1".into()), Some("p2".into()));
        activity.completed(&host, &json!({"requestId":"activity-2","ticketId":"p1-conflict","result":{"ok":false,"error":{"code":"PLUGIN_STORAGE_CONFLICT","message":"changed"}}}));
        activity.context = json!({"activeProject":{"id":"p1"}});
        activity.switch_project(Some("p2".into()), Some("p1".into()));
        assert!(!activity.project_loaded);
        assert_eq!(activity.unsaved, 1);
    }

    #[derive(Default)]
    struct BrokerState {
        calls: Vec<Value>,
        cleanup_error: Option<&'static str>,
        progress: String,
        notifications: BTreeMap<String, bool>,
        snapshot: Value,
    }
    thread_local! {
        static BROKER: std::cell::RefCell<BrokerState> = std::cell::RefCell::default();
    }
    fn recording_host() -> Host {
        use se_plugin_abi::{SeHostApiV1, SeOutput, SeSlice, SEP_ABI_VERSION, SEP_OK};
        unsafe extern "C" fn log(_: *mut core::ffi::c_void, _: u32, _: SeSlice) -> SeStatus {
            SEP_OK
        }
        unsafe extern "C" fn broker(
            _: *mut core::ffi::c_void,
            input: SeSlice,
            output: *mut SeOutput,
        ) -> SeStatus {
            // SAFETY: the ABI supplies valid buffers for the duration of the call.
            let request: Value =
                serde_json::from_slice(unsafe { std::slice::from_raw_parts(input.ptr, input.len) })
                    .unwrap();
            let result = BROKER.with(|cell| {
                let mut state = cell.borrow_mut();
                state.calls.push(request.clone());
                let method = request["method"].as_str().unwrap();
                let input = &request["input"];
                if let Some(code) = state.cleanup_error {
                    if method == "notification.close"
                        || (method == "progress.upsert" && input["state"] == "cancelled")
                    {
                        return json!({"ok":false,"error":{"code":code,"message":"try later"}});
                    }
                }
                match method {
                    "progress.upsert" => state.progress = input["state"].as_str().unwrap().into(),
                    "notification.upsert" | "notification.close" => {
                        state.notifications.insert(
                            input["id"].as_str().unwrap().into(),
                            method == "notification.upsert",
                        );
                    }
                    _ => (),
                }
                let value = if request["service"] == "se.host.storage" {
                    json!({"ticketId":format!("ticket-{}", input["requestId"].as_str().unwrap())})
                } else if method == "snapshot" {
                    state.snapshot.clone()
                } else {
                    Value::Null
                };
                json!({"ok":true,"value":value})
            });
            let bytes = serde_json::to_vec(&json!({"schemaVersion":1,"result":result})).unwrap();
            // SAFETY: Output points to the caller's writable buffer; capacity is checked.
            unsafe {
                let output = &mut *output;
                assert!(output.capacity >= bytes.len());
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), output.ptr, bytes.len());
                output.len = bytes.len();
            }
            SEP_OK
        }
        BROKER.with(|state| *state.borrow_mut() = BrokerState::default());
        let api = SeHostApiV1 {
            abi_version: SEP_ABI_VERSION,
            struct_size: core::mem::size_of::<SeHostApiV1>() as u32,
            host_context: core::ptr::null_mut(),
            log: Some(log),
            broker_call: Some(broker),
        };
        // SAFETY: the callbacks are static and use only thread-local test state.
        unsafe { Host::from_raw(&api) }.unwrap()
    }
    fn finish(activity: &mut Activity, host: &Host, kind: &str, result: Value) {
        let (id, pending) = activity
            .pending
            .iter()
            .find(|(_, pending)| pending.kind == kind)
            .unwrap();
        let completion = json!({"requestId":id,"ticketId":pending.ticket,"result":result});
        activity.completed(host, &completion);
    }
    fn initialized_activity() -> Activity {
        Activity {
            source: "source".into(),
            profile_loaded: true,
            project_loaded: true,
            context: json!({"activeProject":{"id":"p1"},"agentSessions":[{"id":"a1","activeTurn":true,"replaying":false,"status":"active"}]}),
            ..Activity::default()
        }
    }

    #[test]
    fn uncertain_project_set_reconciles_published_delta_before_retry() {
        for error in ["PLUGIN_IO", "PLUGIN_TIMEOUT", "PLUGIN_REVOKED"] {
            for (revision, stored, expected_delta) in [(5, 11, 1), (4, 10, 2), (5, 10, 2)] {
                let host = recording_host();
                let mut activity = initialized_activity();
                activity.project_revision = 4;
                activity.observed = 10;
                activity.unsaved = 1;
                activity.flush(&host);
                // Another end while the first CAS is in flight must not be lost.
                activity.unsaved += 1;
                finish(
                    &mut activity,
                    &host,
                    "project-set",
                    json!({"ok":false,"error":{"code":error}}),
                );
                assert!(!activity.project_loaded);
                assert!(activity.uncertain.is_some());
                finish(
                    &mut activity,
                    &host,
                    "project-get",
                    json!({"ok":true,"value":{"revision":revision,"value":stored}}),
                );
                assert_eq!(activity.unsaved, expected_delta);
                assert_eq!(activity.observed(), 12);
                assert!(activity.uncertain.is_none());
                BROKER.with(|state| {
                    let state = state.borrow();
                    let retry = state
                        .calls
                        .iter()
                        .rfind(|call| {
                            call["service"] == "se.host.storage" && call["method"] == "set"
                        })
                        .unwrap();
                    assert_eq!(retry["input"]["value"], 12);
                    assert_eq!(retry["input"]["expectedRevision"], revision);
                });
            }
        }
    }

    #[test]
    fn disabling_retries_busy_terminal_progress_and_notification_cleanup_on_next_delivery() {
        for error in ["PLUGIN_RATE_LIMITED", "PLUGIN_BUSY"] {
            let host = recording_host();
            let mut activity = initialized_activity();
            activity.present(&host);
            activity
                .command(&host, "example.activity-summary", &Value::Null)
                .unwrap();
            BROKER.with(|state| {
                let mut state = state.borrow_mut();
                assert_eq!(state.progress, "running");
                assert_eq!(state.notifications.get("activity-summary"), Some(&true));
                state
                    .notifications
                    .insert("activity-turn-ended".into(), true);
                state.cleanup_error = Some(error);
            });
            activity
                .command(&host, "example.activity-toggle", &Value::Null)
                .unwrap();
            assert!(!activity.enabled);
            assert_eq!(activity.cleanup, [true, true, true, false]);
            let duplicate = json!({"sourceId":"source","subscriptionId":"turn","sequence":0});
            let before = BROKER.with(|state| state.borrow().calls.len());
            activity.deliver(&host, &duplicate);
            BROKER.with(|state| {
                let mut state = state.borrow_mut();
                assert_eq!(
                    state.calls.len() - before,
                    3,
                    "one attempt per unresolved effect per callback"
                );
                state.cleanup_error = None;
            });
            // Even a duplicate is a retry opportunity, not an initialization prerequisite.
            activity.deliver(&host, &duplicate);
            assert_eq!(activity.cleanup, [false; 4]);
            BROKER.with(|state| {
                let state = state.borrow();
                assert_eq!(state.progress, "cancelled");
                assert_eq!(state.notifications.get("activity-summary"), Some(&false));
                assert_eq!(state.notifications.get("activity-turn-ended"), Some(&false));
            });
            // A later callback may not restore a running card while disabled.
            activity.present(&host);
            BROKER.with(|state| assert_eq!(state.borrow().progress, "cancelled"));
        }
    }

    #[test]
    fn contiguous_turn_end_during_project_get_is_buffered_but_resync_baseline_is_not() {
        let host = recording_host();
        let mut activity = initialized_activity();
        activity.project_loaded = false;
        activity.initialize(&host);
        let turn_end = json!({"sourceId":"source","subscriptionId":"turn","sequence":1,"contextRevision":2,"topic":"agent.session.stateChanged","data":{"session":{"id":"a1","activeTurn":false,"replaying":false,"status":"active"}}});
        activity.deliver(&host, &turn_end);
        activity.deliver(&host, &turn_end);
        assert_eq!(activity.waiting_turns, 1);
        assert_eq!(activity.unsaved, 0);
        BROKER.with(|state| {
            assert!(!state
                .borrow()
                .calls
                .iter()
                .any(|call| call["method"] == "notification.upsert"))
        });
        finish(
            &mut activity,
            &host,
            "project-get",
            json!({"ok":true,"value":{"revision":7,"value":20}}),
        );
        assert_eq!(activity.waiting_turns, 0);
        assert_eq!(activity.unsaved, 1);
        assert_eq!(activity.observed(), 21);
        BROKER.with(|state| {
            let mut state = state.borrow_mut();
            assert_eq!(state.calls.iter().filter(|call| call["method"] == "notification.upsert" && call["input"]["id"] == "activity-turn-ended").count(), 1);
            state.snapshot = json!({"sourceId":"source","contextRevision":9,"sequences":{"turn":3},"context":activity.context});
        });
        // A gap refreshes the idle baseline; it must not manufacture another end.
        let mut gap = turn_end;
        gap["sequence"] = json!(3);
        activity.deliver(&host, &gap);
        assert_eq!(activity.observed(), 21);
        assert_eq!(activity.waiting_turns, 0);
        BROKER.with(|state| {
            assert_eq!(
                state
                    .borrow()
                    .calls
                    .iter()
                    .filter(|call| call["method"] == "notification.upsert"
                        && call["input"]["id"] == "activity-turn-ended")
                    .count(),
                1
            )
        });
    }

    #[test]
    fn activity_starts_loading_and_existing_counter_is_retained() {
        let activity = Activity::default();
        assert_eq!(activity.state(), "Loading activity");
        assert_eq!(activity.observed(), 0);
        for bytes in [
            include_bytes!("manifest.json").as_slice(),
            include_bytes!("manifest-v2.json").as_slice(),
        ] {
            let manifest: Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(manifest["contributes"]["views"][0]["icon"], "calculator");
            assert_eq!(
                manifest["contributes"]["eventSubscriptions"]
                    .as_array()
                    .unwrap()
                    .len(),
                5
            );
            assert_eq!(
                manifest["services"]["requires"].as_array().unwrap().len(),
                8
            );
            assert_eq!(
                manifest["contributes"]["menus"].as_array().unwrap().len(),
                4
            );
            assert_eq!(manifest["contributes"]["keybindings"][0]["key"], "");
            assert!(!String::from_utf8_lossy(bytes).contains("secrets"));
            assert!(!String::from_utf8_lossy(bytes).contains("activity-inspect"));
        }
    }
}
