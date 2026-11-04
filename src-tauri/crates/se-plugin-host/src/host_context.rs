//! Host-owned context and subscription state, accessed under the independent
//! HostServices mutex. No runtime callback, I/O or GUI work occurs here.
//! Timestamps are host supplied and monotonically clamped for deterministic tests.
use se_plugin_protocol::*;
use std::{
    borrow::Borrow,
    collections::{BTreeMap, VecDeque},
};

type Result<T> = std::result::Result<T, PluginError>;
struct Source {
    owner: String,
    id: String,
    sequence: u64,
    last_seen: u64,
    context: HostContextState,
    needs_reset: bool,
}
struct Subscription {
    declaration: EventSubscriptionContribution,
    sequence: u64,
    acknowledged: u64,
    resync_at: Option<u64>,
    marker_pending: bool,
}
struct Lease {
    generation: String,
    active: bool,
    subscriptions: Vec<Subscription>,
    queue: VecDeque<HostEventDeliveryInput>,
}
/// Self-contained state. HostServices owns the one-inflight delivery policy.
pub struct HostContext {
    source: Option<Source>,
    revision: u64,
    last_now: u64,
    leases: BTreeMap<String, Lease>,
    max_pending: usize,
    source_ttl_ms: u64,
}
fn error(code: &str, message: &str) -> PluginError {
    PluginError::new(code, message)
}
fn unavailable() -> PluginError {
    error(
        PLUGIN_SERVICE_UNAVAILABLE,
        "host context source is unavailable",
    )
}
fn stale() -> PluginError {
    error(PLUGIN_STALE_GENERATION, "host context generation is stale")
}
fn next(value: u64) -> Result<u64> {
    value
        .checked_add(1)
        .filter(|n| *n <= MAX_SAFE_INTEGER)
        .ok_or_else(|| error(PLUGIN_BUSY, "host context sequence exhausted"))
}
fn project(context: &HostContextState) -> Option<&str> {
    context.active_project.as_ref().map(|p| p.id.as_str())
}
fn matches_subscription(
    declaration: &EventSubscriptionContribution,
    event: &HostSourceEvent,
    context: &HostContextState,
) -> bool {
    declaration.topic == event.topic
        && (declaration.scope == HostScope::Window
            || (event.project_id.is_some() && event.project_id.as_deref() == project(context)))
}
fn scoped(mut context: HostContextState, scope: HostScope) -> HostContextState {
    if scope == HostScope::ActiveProject {
        let id = project(&context).map(str::to_owned);
        context.project_ids.retain(|p| Some(p) == id.as_ref());
        context
            .terminals
            .retain(|t| id.is_some() && t.project_id == id);
        context
            .agent_sessions
            .retain(|s| Some(&s.project_id) == id.as_ref());
        if !context
            .terminals
            .iter()
            .any(|t| Some(&t.id) == context.active_terminal_id.as_ref())
        {
            context.active_terminal_id = None;
        }
        if !context
            .agent_sessions
            .iter()
            .any(|s| Some(&s.id) == context.active_session_id.as_ref())
        {
            context.active_session_id = None;
        }
    }
    context
}
impl Default for HostContext {
    fn default() -> Self {
        Self::new()
    }
}
impl HostContext {
    pub fn new() -> Self {
        Self::with_limits(128, 10_000)
    }
    /// Policy is explicit to allow deterministic boundary tests.
    pub fn with_limits(max_pending: usize, source_ttl_ms: u64) -> Self {
        assert!(max_pending > 0 && source_ttl_ms > 0);
        Self {
            source: None,
            revision: 0,
            last_now: 0,
            leases: BTreeMap::new(),
            max_pending,
            source_ttl_ms,
        }
    }
    pub fn register(&mut self, manifest: &Manifest, generation: &str) -> Result<()> {
        if !is_uuid(generation) {
            return Err(error(PLUGIN_INVALID_REQUEST, "invalid context generation"));
        }
        if self.leases.contains_key(&manifest.id) {
            return Err(error(PLUGIN_BUSY, "context generation already registered"));
        }
        self.leases.insert(
            manifest.id.clone(),
            Lease {
                generation: generation.into(),
                active: false,
                queue: VecDeque::new(),
                subscriptions: manifest
                    .contributes
                    .event_subscriptions
                    .iter()
                    .cloned()
                    .map(|declaration| Subscription {
                        declaration,
                        sequence: 0,
                        acknowledged: 0,
                        resync_at: None,
                        marker_pending: false,
                    })
                    .collect(),
            },
        );
        Ok(())
    }
    pub fn activate(&mut self, plugin: &str, generation: &str) -> Result<()> {
        let lease = self
            .leases
            .get_mut(plugin)
            .filter(|l| l.generation == generation)
            .ok_or_else(stale)?;
        if lease.active {
            return Ok(());
        }
        for s in &lease.subscriptions {
            next(s.sequence)?;
        }
        lease.active = true;
        for s in &mut lease.subscriptions {
            s.sequence += 1;
            s.resync_at = Some(s.sequence);
            s.marker_pending = true;
        }
        Ok(())
    }
    pub fn revoke(&mut self, plugin: &str, generation: &str) {
        if self
            .leases
            .get(plugin)
            .is_some_and(|l| l.generation == generation)
        {
            self.leases.remove(plugin);
        }
    }
    /// Cleanup diagnostics consumed by embedding-host verification.
    #[allow(dead_code)]
    pub fn resource_count(&self) -> usize {
        self.leases
            .values()
            .map(|l| l.subscriptions.len() + l.queue.len())
            .sum()
    }
    /// Number of live declarations, including provisional generations.
    #[allow(dead_code)]
    pub fn subscription_count(&self) -> usize {
        self.leases.values().map(|l| l.subscriptions.len()).sum()
    }
    fn clock(&mut self, now: u64) -> u64 {
        self.last_now = self.last_now.max(now);
        self.last_now
    }
    pub fn expire(&mut self, now: u64) -> bool {
        let now = self.clock(now);
        if self
            .source
            .as_ref()
            .is_some_and(|s| now.saturating_sub(s.last_seen) >= self.source_ttl_ms)
        {
            self.source = None;
            for lease in self.leases.values_mut() {
                lease.queue.clear();
            }
            return true;
        }
        false
    }
    pub fn detach(&mut self, owner: &str) -> bool {
        if self.source.as_ref().is_none_or(|s| s.owner != owner) {
            return false;
        }
        self.source = None;
        for lease in self.leases.values_mut() {
            lease.queue.clear();
        }
        true
    }
    /// Raw context access for other host services; does NOT acknowledge events.
    pub fn current(&mut self, now: u64) -> Result<(u64, HostContextState)> {
        self.expire(now);
        Ok((
            self.revision,
            self.source
                .as_ref()
                .filter(|source| !source.needs_reset)
                .ok_or_else(unavailable)?
                .context
                .clone(),
        ))
    }
    /// Web reads must never consume the native queue, watermarks or resync markers.
    pub fn peek_snapshot(
        &self,
        plugin: &str,
        generation: &str,
        scope: HostScope,
        now: u64,
    ) -> Result<HostContextSnapshot> {
        let lease = self
            .leases
            .get(plugin)
            .filter(|l| l.generation == generation)
            .ok_or_else(stale)?;
        let source = self
            .source
            .as_ref()
            .filter(|source| {
                !source.needs_reset
                    && now.max(self.last_now).saturating_sub(source.last_seen) < self.source_ttl_ms
            })
            .ok_or_else(unavailable)?;
        Ok(HostContextSnapshot {
            source_id: source.id.clone(),
            context_revision: self.revision,
            scope,
            context: scoped(source.context.clone(), scope),
            sequences: lease
                .subscriptions
                .iter()
                .map(|s| (s.declaration.id.clone(), s.sequence))
                .collect(),
        })
    }
    pub fn snapshot(
        &mut self,
        plugin: &str,
        generation: &str,
        scope: HostScope,
        now: u64,
    ) -> Result<HostContextSnapshot> {
        self.expire(now);
        let snapshot = self.peek_snapshot(plugin, generation, scope, now)?;
        let lease = self.leases.get_mut(plugin).expect("validated lease");
        for s in &mut lease.subscriptions {
            s.acknowledged = s.sequence;
            s.resync_at = None;
            s.marker_pending = false;
        }
        lease.queue.clear();
        Ok(snapshot)
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    /// Independent web copies use declaration matching, not native pending/ack state.
    /// Their sequence is assigned by the destination web session before enqueueing.
    pub(crate) fn web_events(
        &self,
        plugin: &str,
        generation: &str,
        request: &HostContextPublishRequest,
    ) -> Vec<HostEventDeliveryInput> {
        let Some(lease) = self
            .leases
            .get(plugin)
            .filter(|l| l.active && l.generation == generation)
        else {
            return Vec::new();
        };
        let mut deliveries = Vec::new();
        for event in &request.events {
            for sub in &lease.subscriptions {
                if matches_subscription(&sub.declaration, event, &request.snapshot) {
                    deliveries.push(HostEventDeliveryInput {
                        generation: generation.into(),
                        subscription_id: sub.declaration.id.clone(),
                        source_id: request.source_id.clone(),
                        sequence: self.revision,
                        context_revision: self.revision,
                        topic: event.topic,
                        project_id: event.project_id.clone(),
                        resync_required: false,
                        data: Some(event.data.clone()),
                    });
                }
            }
        }
        deliveries
    }
    pub fn heartbeat(&mut self, owner: &str, source_id: &str, now: u64) -> Result<HostContextAck> {
        let now = self.clock(now);
        self.expire(now);
        let source = self.source.as_mut().ok_or_else(unavailable)?;
        if source.owner != owner || source.id != source_id {
            return Err(error(
                PLUGIN_BUSY,
                "context source is bound to another owner or source",
            ));
        }
        if source.needs_reset {
            return Err(error(
                PLUGIN_INVALID_REQUEST,
                "context source sequence gap requires reset",
            ));
        }
        source.last_seen = now;
        Ok(HostContextAck {
            context_revision: self.revision,
            source_sequence: source.sequence,
        })
    }
    pub fn publish<R: Borrow<HostContextPublishRequest>>(
        &mut self,
        owner: &str,
        request: R,
        now: u64,
    ) -> Result<HostContextAck> {
        let request = request.borrow();
        validate_host_context_publish_request(request)?;
        if owner.is_empty() {
            return Err(error(
                PLUGIN_INVALID_REQUEST,
                "missing context source owner",
            ));
        }
        let now = self.clock(now);
        self.expire(now);
        if let Some(source) = &self.source {
            if source.owner != owner || (source.id != request.source_id && !request.reset) {
                return Err(error(
                    PLUGIN_BUSY,
                    "context source is bound to another owner or source",
                ));
            }
            if source.id == request.source_id {
                if request.source_sequence <= source.sequence {
                    return Ok(HostContextAck {
                        source_sequence: source.sequence,
                        context_revision: self.revision,
                    });
                }
                if !request.reset
                    && (source.needs_reset || request.source_sequence != next(source.sequence)?)
                {
                    self.source.as_mut().expect("source checked").needs_reset = true;
                    for lease in self.leases.values_mut().filter(|l| l.active) {
                        lease.queue.clear();
                        for s in &mut lease.subscriptions {
                            s.sequence = next(s.sequence)?;
                            s.resync_at = Some(s.sequence);
                            s.marker_pending = true;
                        }
                    }
                    return Err(error(
                        PLUGIN_INVALID_REQUEST,
                        "context source sequence gap requires reset",
                    ));
                }
            }
        } else if !request.reset {
            return Err(error(
                PLUGIN_INVALID_REQUEST,
                "new context source requires reset",
            ));
        }
        let revision = next(self.revision)?;
        // Reserve enough sequence headroom before any mutation.
        for lease in self.leases.values() {
            for s in &lease.subscriptions {
                if s.sequence > MAX_SAFE_INTEGER.saturating_sub(request.events.len() as u64 + 1) {
                    return Err(error(PLUGIN_BUSY, "host context sequence exhausted"));
                }
            }
        }
        let switched = self
            .source
            .as_ref()
            .is_some_and(|s| project(&s.context) != project(&request.snapshot));
        for lease in self.leases.values_mut().filter(|l| l.active) {
            if request.reset {
                lease.queue.clear();
            }
            if switched {
                lease.queue.retain(|e| {
                    !lease.subscriptions.iter().any(|s| {
                        s.declaration.id == e.subscription_id
                            && s.declaration.scope == HostScope::ActiveProject
                    })
                });
            }
            for index in 0..lease.subscriptions.len() {
                let declaration = &lease.subscriptions[index].declaration;
                if request.reset || (switched && declaration.scope == HostScope::ActiveProject) {
                    enqueue(
                        lease,
                        index,
                        &request.source_id,
                        revision,
                        project(&request.snapshot).map(str::to_owned),
                        None,
                        self.max_pending,
                    )?;
                }
            }
            // Transitions are not inferred from a snapshot: create+close within
            // one publish must still reach lifecycle subscribers in source order.
            for event in &request.events {
                for index in 0..lease.subscriptions.len() {
                    let sub = &lease.subscriptions[index];
                    if matches_subscription(&sub.declaration, event, &request.snapshot) {
                        enqueue(
                            lease,
                            index,
                            &request.source_id,
                            revision,
                            event.project_id.clone(),
                            Some(event.data.clone()),
                            self.max_pending,
                        )?;
                    }
                }
            }
        }
        self.revision = revision;
        self.source = Some(Source {
            owner: owner.into(),
            id: request.source_id.clone(),
            sequence: request.source_sequence,
            last_seen: now,
            context: request.snapshot.clone(),
            needs_reset: false,
        });
        Ok(HostContextAck {
            source_sequence: request.source_sequence,
            context_revision: revision,
        })
    }
    pub fn pop(
        &mut self,
        plugin: &str,
        generation: &str,
        now: u64,
    ) -> Option<HostEventDeliveryInput> {
        self.expire(now);
        let source = self.source.as_ref().filter(|source| !source.needs_reset)?;
        let lease = self
            .leases
            .get_mut(plugin)
            .filter(|l| l.active && l.generation == generation)?;
        if let Some(sub) = lease.subscriptions.iter_mut().find(|s| s.marker_pending) {
            sub.marker_pending = false;
            return Some(HostEventDeliveryInput {
                generation: generation.into(),
                subscription_id: sub.declaration.id.clone(),
                source_id: source.id.clone(),
                sequence: sub.sequence,
                context_revision: self.revision,
                topic: sub.declaration.topic,
                project_id: project(&source.context).map(str::to_owned),
                resync_required: true,
                data: None,
            });
        }
        lease.queue.pop_front()
    }
    pub fn put_back(&mut self, plugin: &str, generation: &str, event: HostEventDeliveryInput) {
        let Some(lease) = self
            .leases
            .get_mut(plugin)
            .filter(|l| l.active && l.generation == generation && l.generation == event.generation)
        else {
            return;
        };
        if self.source.as_ref().is_none_or(|s| s.id != event.source_id) {
            return;
        }
        let Some(sub) = lease
            .subscriptions
            .iter_mut()
            .find(|s| s.declaration.id == event.subscription_id)
        else {
            return;
        };
        if event.resync_required {
            if sub.resync_at.is_some() {
                sub.marker_pending = true;
            }
            return;
        }
        // A snapshot may have acknowledged the event while it was in flight.
        if sub.resync_at.is_some() || event.sequence <= sub.acknowledged {
            return;
        }
        let bytes = lease
            .queue
            .iter()
            .chain(std::iter::once(&event))
            .map(|e| serde_json::to_vec(e).map_or(usize::MAX, |v| v.len()))
            .fold(0usize, usize::saturating_add);
        if lease.queue.len() < self.max_pending && bytes <= 1024 * 1024 {
            lease.queue.push_front(event);
        } else {
            lease.queue.clear();
            for s in &mut lease.subscriptions {
                s.resync_at = Some(s.sequence);
                s.marker_pending = true;
            }
        }
    }
}

fn enqueue(
    lease: &mut Lease,
    index: usize,
    source_id: &str,
    revision: u64,
    project_id: Option<String>,
    data: Option<HostEventData>,
    max_pending: usize,
) -> Result<()> {
    let sub = &mut lease.subscriptions[index];
    sub.sequence = next(sub.sequence)?;
    if data.is_none() || sub.resync_at.is_some() {
        sub.resync_at = Some(sub.sequence);
        sub.marker_pending = true;
        lease
            .queue
            .retain(|e| e.subscription_id != sub.declaration.id);
        return Ok(());
    }
    let event = HostEventDeliveryInput {
        subscription_id: sub.declaration.id.clone(),
        generation: lease.generation.clone(),
        source_id: source_id.into(),
        sequence: sub.sequence,
        context_revision: revision,
        topic: sub.declaration.topic,
        project_id,
        data,
        resync_required: false,
    };
    let lifecycle = matches!(
        event.topic,
        HostEventTopic::TerminalCreated | HostEventTopic::TerminalClosed
    );
    if !lifecycle {
        let queued = lease.queue.len();
        lease.queue.retain(|old| {
            !(old.subscription_id == event.subscription_id
                && old.project_id == event.project_id
                && (event.topic != HostEventTopic::AgentSessionStateChanged
                    || match (&old.data, &event.data) {
                        (
                            Some(HostEventData::AgentSessionStateChanged(a)),
                            Some(HostEventData::AgentSessionStateChanged(b)),
                        ) => a.session.id == b.session.id,
                        _ => false,
                    }))
        });
        if lease.queue.len() != queued {
            sub.resync_at = Some(sub.sequence);
            sub.marker_pending = true;
            lease
                .queue
                .retain(|e| e.subscription_id != sub.declaration.id);
            return Ok(());
        }
    }
    let bytes = serde_json::to_vec(&event).map_or(usize::MAX, |v| v.len());
    let queued_bytes = lease
        .queue
        .iter()
        .map(|e| serde_json::to_vec(e).map_or(usize::MAX, |v| v.len()))
        .fold(0usize, usize::saturating_add);
    if lease.queue.len() >= max_pending || queued_bytes.saturating_add(bytes) > 1024 * 1024 {
        lease.queue.clear();
        for s in &mut lease.subscriptions {
            s.resync_at = Some(s.sequence);
            s.marker_pending = true;
        }
    } else {
        lease.queue.push_back(event);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const GEN: &str = "11111111-1111-4111-8111-111111111111";
    const SOURCE: &str = "22222222-2222-4222-8222-222222222222";
    fn state(p: &str) -> HostContextState {
        serde_json::from_value(serde_json::json!({
            "activeProject": {"id": p, "name": p, "path": "/project"},
            "projectIds": [p], "activeTerminalId": null, "activeSessionId": null,
            "terminals": [], "agentSessions": [], "focus": {"textInput": false, "terminal": false}, "truncated": false
        })).unwrap()
    }
    fn request(
        sequence: u64,
        reset: bool,
        p: &str,
        events: Vec<HostSourceEvent>,
    ) -> HostContextPublishRequest {
        HostContextPublishRequest {
            source_id: SOURCE.into(),
            source_sequence: sequence,
            reset,
            snapshot: state(p),
            events,
        }
    }
    fn created(p: &str, id: &str) -> HostSourceEvent {
        serde_json::from_value(
            serde_json::json!({ "topic": "terminal.created", "projectId": p,
            "data": {"terminal": {"id": id, "ptyId": null, "projectId": p}} }),
        )
        .unwrap()
    }
    fn host(cap: usize) -> HostContext {
        let mut host = HostContext::with_limits(cap, 10_000);
        let declaration = EventSubscriptionContribution {
            id: "created".into(),
            topic: HostEventTopic::TerminalCreated,
            scope: HostScope::ActiveProject,
        };
        host.leases.insert(
            "demo".into(),
            Lease {
                generation: GEN.into(),
                active: false,
                queue: VecDeque::new(),
                subscriptions: vec![Subscription {
                    declaration,
                    sequence: 0,
                    acknowledged: 0,
                    resync_at: None,
                    marker_pending: false,
                }],
            },
        );
        host.activate("demo", GEN).unwrap();
        host.publish("window", request(1, true, "a", vec![]), 0)
            .unwrap();
        host.snapshot("demo", GEN, HostScope::Window, 0).unwrap();
        host
    }
    #[test]
    fn web_snapshot_and_event_copy_leave_native_queue_ack_and_resync_untouched() {
        fn native_state(host: &HostContext) -> serde_json::Value {
            let lease = &host.leases["demo"];
            serde_json::json!({
                "queue": lease.queue,
                "subscriptions": lease.subscriptions.iter().map(|s| (
                    s.sequence, s.acknowledged, s.resync_at, s.marker_pending
                )).collect::<Vec<_>>()
            })
        }
        let mut host = host(2);
        for (sequence, count) in [(2, 1), (3, 3)] {
            let request = request(
                sequence,
                false,
                "a",
                (0..count).map(|_| created("a", "t")).collect(),
            );
            host.publish("window", &request, sequence).unwrap();
            let before = native_state(&host);
            let snapshot = host
                .peek_snapshot("demo", GEN, HostScope::ActiveProject, sequence)
                .unwrap();
            assert_eq!(
                snapshot.sequences["created"],
                host.leases["demo"].subscriptions[0].sequence
            );
            assert_eq!(host.web_events("demo", GEN, &request).len(), count);
            assert_eq!(native_state(&host), before);
            if sequence == 2 {
                assert_eq!(host.leases["demo"].queue.len(), 1);
            } else {
                assert!(host.leases["demo"].subscriptions[0].resync_at.is_some());
                assert!(host.leases["demo"].subscriptions[0].marker_pending);
            }
        }
        let before = native_state(&host);
        assert_eq!(
            host.peek_snapshot("demo", GEN, HostScope::Window, 10_003)
                .unwrap_err()
                .code,
            PLUGIN_SERVICE_UNAVAILABLE
        );
        assert_eq!(native_state(&host), before);
        // Only a native snapshot acknowledges and clears the overflow marker.
        host.snapshot("demo", GEN, HostScope::Window, 3).unwrap();
        let sub = &host.leases["demo"].subscriptions[0];
        assert_eq!(sub.acknowledged, sub.sequence);
        assert!(sub.resync_at.is_none());
        assert!(!sub.marker_pending);
    }

    #[test]
    fn same_owner_can_reset_to_new_source_without_waiting_for_expiry() {
        let mut host = host(128);
        host.publish(
            "window",
            request(2, false, "a", vec![created("a", "old")]),
            1,
        )
        .unwrap();
        let mut replacement = request(1, true, "b", vec![]);
        replacement.source_id = GEN.into();
        let mut without_reset = replacement.clone();
        without_reset.reset = false;
        assert_eq!(
            host.publish("window", without_reset, 2).unwrap_err().code,
            PLUGIN_BUSY
        );
        assert_eq!(
            host.publish("other-window", replacement.clone(), 2)
                .unwrap_err()
                .code,
            PLUGIN_BUSY
        );
        host.publish("window", replacement, 2).unwrap();
        assert_eq!(host.source.as_ref().unwrap().id, GEN);
        assert_eq!(host.source.as_ref().unwrap().sequence, 1);
        assert!(host.leases["demo"].queue.is_empty());
        let marker = host.pop("demo", GEN, 2).unwrap();
        assert_eq!(marker.source_id, GEN);
        assert!(marker.resync_required);
        let snapshot = host.snapshot("demo", GEN, HostScope::Window, 2).unwrap();
        assert_eq!(project(&snapshot.context), Some("b"));
        assert_eq!(snapshot.sequences["created"], marker.sequence);
    }

    #[test]
    fn event_overflow_requires_snapshot_watermark() {
        let mut host = host(2);
        host.publish(
            "window",
            request(
                2,
                false,
                "a",
                vec![created("a", "t1"), created("a", "t2"), created("a", "t3")],
            ),
            1,
        )
        .unwrap();
        assert!(host.leases["demo"].queue.is_empty());
        let marker = host.pop("demo", GEN, 1).unwrap();
        assert!(marker.resync_required && marker.data.is_none());
        assert!(host.pop("demo", GEN, 1).is_none());
        host.publish(
            "window",
            request(3, false, "a", vec![created("a", "t4")]),
            2,
        )
        .unwrap();
        assert!(host.pop("demo", GEN, 2).unwrap().resync_required);
        let snapshot = host.snapshot("demo", GEN, HostScope::Window, 2).unwrap();
        assert!(snapshot.sequences["created"] > marker.sequence);
        host.put_back("demo", GEN, marker);
        assert!(host.pop("demo", GEN, 2).is_none());
        host.publish(
            "window",
            request(4, false, "a", vec![created("a", "t5")]),
            3,
        )
        .unwrap();
        let event = host.pop("demo", GEN, 3).unwrap();
        assert!(!event.resync_required && event.data.is_some());
        assert!(event.sequence > snapshot.sequences["created"]);
    }
    #[test]
    fn project_switch_resets_scoped_subscriptions() {
        let mut host = host(128);
        host.publish(
            "window",
            request(2, false, "a", vec![created("a", "old")]),
            1,
        )
        .unwrap();
        host.publish(
            "window",
            request(
                3,
                false,
                "b",
                vec![created("a", "late"), created("b", "new")],
            ),
            2,
        )
        .unwrap();
        let marker = host.pop("demo", GEN, 2).unwrap();
        assert_eq!(marker.project_id.as_deref(), Some("b"));
        assert!(marker.resync_required);
        assert!(host.pop("demo", GEN, 2).is_none());
        let snapshot = host
            .snapshot("demo", GEN, HostScope::ActiveProject, 2)
            .unwrap();
        assert_eq!(project(&snapshot.context), Some("b"));
        assert_eq!(snapshot.sequences["created"], marker.sequence);
        host.publish(
            "window",
            request(
                4,
                false,
                "b",
                vec![created("a", "late2"), created("b", "new2")],
            ),
            3,
        )
        .unwrap();
        assert_eq!(
            host.pop("demo", GEN, 3).unwrap().project_id.as_deref(),
            Some("b")
        );
        assert!(host.pop("demo", GEN, 3).is_none());
    }
    #[test]
    fn owner_binding_reset_heartbeat_expiry() {
        let mut host = host(128);
        assert_eq!(
            host.publish("other", request(2, true, "b", vec![]), 1)
                .unwrap_err()
                .code,
            PLUGIN_BUSY
        );
        assert!(!host.detach("other"));
        assert!(host.heartbeat("other", SOURCE, 2).is_err());
        assert!(host
            .publish("window", request(3, false, "a", vec![]), 3)
            .is_err());
        assert!(host.pop("demo", GEN, 3).is_none());
        assert!(host.heartbeat("window", SOURCE, 3).is_err());
        assert!(host
            .publish("window", request(2, false, "a", vec![]), 4)
            .is_err());
        host.publish("window", request(4, true, "b", vec![]), 5)
            .unwrap();
        let revision = host.revision;
        host.publish("window", request(4, true, "a", vec![]), 6)
            .unwrap();
        assert_eq!(host.revision, revision);
        assert_eq!(project(&host.current(6).unwrap().1), Some("b"));
        host.heartbeat("window", SOURCE, 9_000).unwrap();
        assert!(!host.expire(18_999));
        assert!(host.expire(19_000));
        assert_eq!(
            host.snapshot("demo", GEN, HostScope::Window, 19_000)
                .unwrap_err()
                .code,
            PLUGIN_SERVICE_UNAVAILABLE
        );
        assert!(host.heartbeat("window", SOURCE, 19_000).is_err());
        assert!(host
            .publish("window", request(5, false, "b", vec![]), 19_000)
            .is_err());
        host.publish("other", request(1, true, "a", vec![]), 19_000)
            .unwrap();
        assert!(host.detach("other"));
        assert!(host.current(19_000).is_err());
    }
    #[test]
    fn rejected_gap_blocks_reads_and_delivery_until_valid_reset() {
        let mut host = host(128);
        host.publish(
            "window",
            request(2, false, "a", vec![created("a", "queued")]),
            1,
        )
        .unwrap();
        let acknowledged = host.leases["demo"].subscriptions[0].acknowledged;
        assert_eq!(
            host.publish("window", request(4, false, "b", vec![]), 2)
                .unwrap_err()
                .code,
            PLUGIN_INVALID_REQUEST
        );
        let sequence = host.leases["demo"].subscriptions[0].sequence;
        assert!(host.source.as_ref().unwrap().needs_reset);
        assert_eq!(
            host.current(2).unwrap_err().code,
            PLUGIN_SERVICE_UNAVAILABLE
        );
        assert_eq!(
            host.snapshot("demo", GEN, HostScope::Window, 2)
                .unwrap_err()
                .code,
            PLUGIN_SERVICE_UNAVAILABLE
        );
        let sub = &host.leases["demo"].subscriptions[0];
        assert_eq!(sub.acknowledged, acknowledged);
        assert_eq!(sub.resync_at, Some(sequence));
        assert!(sub.marker_pending);
        assert!(host.pop("demo", GEN, 2).is_none());
        assert!(host.leases["demo"].subscriptions[0].marker_pending);
        // Neither an old reset nor a non-reset publish repairs a rejected gap.
        host.publish("window", request(2, true, "b", vec![]), 3)
            .unwrap();
        assert!(host.current(3).is_err());
        assert!(host.pop("demo", GEN, 3).is_none());
        assert!(host
            .publish("window", request(3, false, "a", vec![]), 4)
            .is_err());
        assert!(host.pop("demo", GEN, 4).is_none());
        host.publish("window", request(5, true, "b", vec![]), 5)
            .unwrap();
        assert_eq!(project(&host.current(5).unwrap().1), Some("b"));
        let marker = host.pop("demo", GEN, 5).unwrap();
        assert!(marker.resync_required && marker.data.is_none());
        assert!(marker.sequence > sequence);
        let snapshot = host.snapshot("demo", GEN, HostScope::Window, 5).unwrap();
        assert_eq!(project(&snapshot.context), Some("b"));
        assert_eq!(snapshot.sequences["created"], marker.sequence);
        assert_eq!(
            host.leases["demo"].subscriptions[0].acknowledged,
            marker.sequence
        );
        host.publish(
            "window",
            request(6, false, "b", vec![created("b", "new")]),
            6,
        )
        .unwrap();
        let event = host.pop("demo", GEN, 6).unwrap();
        assert!(!event.resync_required && event.data.is_some());
        assert!(event.sequence > snapshot.sequences["created"]);
    }
    #[test]
    fn latest_value_coalescing_and_byte_overflow() {
        let mut host = host(128);
        let lease = host.leases.get_mut("demo").unwrap();
        lease.subscriptions[0].declaration.topic = HostEventTopic::TerminalActiveChanged;
        let active = |id: &str| {
            serde_json::from_value::<HostSourceEvent>(serde_json::json!({
            "topic": "terminal.activeChanged", "projectId": "a", "data": {"terminalId": id, "projectId": "a"}
        })).unwrap()
        };
        let before = host.leases["demo"].subscriptions[0].sequence;
        host.publish(
            "window",
            request(
                2,
                false,
                "a",
                vec![active("one"), active("two"), active("three")],
            ),
            1,
        )
        .unwrap();
        assert!(host.leases["demo"].queue.is_empty());
        let marker = host.pop("demo", GEN, 1).unwrap();
        assert!(marker.resync_required && marker.data.is_none());
        assert_eq!(marker.sequence, before + 3);
        assert_eq!(
            serde_json::to_value(&marker).unwrap()["data"],
            serde_json::Value::Null
        );
        assert!(host.pop("demo", GEN, 1).is_none());
        host.publish("window", request(3, false, "a", vec![active("four")]), 2)
            .unwrap();
        let latest = host.pop("demo", GEN, 2).unwrap();
        assert!(latest.resync_required && latest.data.is_none());
        assert_eq!(latest.sequence, marker.sequence + 1);
        assert!(host.leases["demo"].queue.is_empty());
        let snapshot = host.snapshot("demo", GEN, HostScope::Window, 2).unwrap();
        assert_eq!(snapshot.sequences["created"], latest.sequence);
        assert!(host.leases["demo"].subscriptions[0].resync_at.is_none());
        host.put_back("demo", GEN, marker);
        assert!(host.pop("demo", GEN, 2).is_none());
        host.publish("window", request(4, false, "a", vec![active("five")]), 3)
            .unwrap();
        let event = host.pop("demo", GEN, 3).unwrap();
        assert!(!event.resync_required);
        assert_eq!(event.data, Some(active("five").data));
        assert!(event.sequence > snapshot.sequences["created"]);
        assert!(host.pop("demo", GEN, 3).is_none());
        // Exercise the total-byte queue admission separately from the source DTO
        // parser (which deliberately rejects an individual oversized payload).
        let huge = HostEventData::TerminalLifecycle(TerminalLifecycleData {
            terminal: TerminalContext {
                id: "x".repeat(1024 * 1024),
                pty_id: None,
                project_id: Some("a".into()),
            },
        });
        let lease = host.leases.get_mut("demo").unwrap();
        enqueue(lease, 0, SOURCE, 3, Some("a".into()), Some(huge), 128).unwrap();
        assert!(lease.queue.is_empty());
        assert!(host.pop("demo", GEN, 1).unwrap().resync_required);
    }
    #[test]
    fn scope_filtering_and_generation_revocation() {
        let mut host = host(128);
        let source = host.source.as_mut().unwrap();
        source.context.project_ids.push("b".into());
        source.context.terminals.push(TerminalContext {
            id: "a-terminal".into(),
            pty_id: None,
            project_id: Some("a".into()),
        });
        source.context.terminals.push(TerminalContext {
            id: "b-terminal".into(),
            pty_id: None,
            project_id: Some("b".into()),
        });
        source.context.active_terminal_id = Some("b-terminal".into());
        let filtered = host
            .snapshot("demo", GEN, HostScope::ActiveProject, 1)
            .unwrap();
        assert_eq!(filtered.context.project_ids, vec!["a"]);
        assert_eq!(filtered.context.terminals.len(), 1);
        assert!(filtered.context.active_terminal_id.is_none());
        host.revoke("demo", "33333333-3333-4333-8333-333333333333");
        assert!(host.leases.contains_key("demo"));
        host.revoke("demo", GEN);
        assert!(host.pop("demo", GEN, 1).is_none());
        assert_eq!(
            host.snapshot("demo", GEN, HostScope::Window, 1)
                .unwrap_err()
                .code,
            PLUGIN_STALE_GENERATION
        );
    }
    #[test]
    fn lifecycle_fifo_and_raw_reads_do_not_acknowledge() {
        let mut host = host(128);
        host.publish(
            "window",
            request(
                2,
                false,
                "a",
                vec![created("a", "one"), created("a", "two")],
            ),
            1,
        )
        .unwrap();
        let first = host.pop("demo", GEN, 1).unwrap();
        host.put_back("demo", GEN, first.clone());
        assert_eq!(host.pop("demo", GEN, 1).unwrap(), first);
        assert!(host.pop("demo", GEN, 1).unwrap().sequence > first.sequence);
        host.publish("window", request(3, true, "a", vec![]), 2)
            .unwrap();
        host.current(2).unwrap();
        assert!(host.pop("demo", GEN, 2).unwrap().resync_required);
        host.publish(
            "window",
            request(4, false, "a", vec![created("a", "three")]),
            3,
        )
        .unwrap();
        assert!(host.pop("demo", GEN, 3).unwrap().resync_required);
    }
}
