//! Authoritative per-instance state. Raw hook payloads never enter this store.
use super::contract::*;
use super::HookKind;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const DELTA_LIMIT: usize = 512;
const DELTA_TTL: Duration = Duration::from_secs(300);
const ENDED_LIMIT: usize = 512;
const ENDED_TTL: Duration = Duration::from_secs(600);
const DEBUG_LIMIT: usize = 128;

#[derive(Clone)]
pub struct StateInput {
    pub terminal_id: String,
    pub generation: u64,
    pub adapter_install_id: String,
    pub project_id: String,
    pub identity: HookIdentityDto,
    pub kind: HookKind,
}
#[derive(Clone)]
struct Change {
    at: Instant,
    project: String,
    revision: u64,
    state: Option<HookStateDto>,
    tombstone: Option<HookStateTombstone>,
}
#[derive(Default, Clone, Debug)]
pub struct Counters {
    pub hook_received: u64,
    pub state_upserts: u64,
    pub state_broadcasts: u64,
    // This memory-only reducer performs neither event logging nor disk I/O.
    pub per_event_logs: u64,
    pub disk_writes: u64,
}
pub struct StateStore {
    instance: String,
    revision: u64,
    states: HashMap<String, HookStateDto>,
    ended: HashMap<String, Instant>,
    changes: VecDeque<Change>,
    debug_enabled: bool,
    // Only a revision and semantic kind, never payload, path, token or session ID.
    debug: VecDeque<(Instant, u64, HookKind)>,
    pub counters: Counters,
}
impl StateStore {
    pub fn new(instance: String) -> Self {
        Self {
            instance,
            revision: 0,
            states: HashMap::new(),
            ended: HashMap::new(),
            changes: VecDeque::new(),
            debug_enabled: false,
            debug: VecDeque::new(),
            counters: Counters::default(),
        }
    }
    fn trim(&mut self, now: Instant) {
        while self
            .changes
            .front()
            .is_some_and(|c| now.duration_since(c.at) > DELTA_TTL)
        {
            self.changes.pop_front();
        }
        while self
            .debug
            .front()
            .is_some_and(|c| now.duration_since(c.0) > DELTA_TTL)
        {
            self.debug.pop_front();
        }
    }
    fn record(&mut self, change: Change) {
        self.changes.push_back(change);
        while self.changes.len() > DELTA_LIMIT {
            self.changes.pop_front();
        }
    }
    pub fn set_debug(&mut self, enabled: bool) {
        self.debug_enabled = enabled;
        if !enabled {
            self.debug.clear();
        }
    }
    pub fn upsert(&mut self, input: StateInput, now: Instant) -> Option<HookStateDto> {
        self.counters.hook_received += 1;
        self.trim(now);
        if self.debug_enabled {
            self.debug.push_back((now, self.revision, input.kind));
            while self.debug.len() > DEBUG_LIMIT {
                self.debug.pop_front();
            }
        }
        if matches!(input.kind, HookKind::ToolStart | HookKind::ToolEnd) {
            return None;
        }
        let old = self.states.get(&input.terminal_id);
        if let Some(old) = old {
            if input.generation < old.generation {
                return None;
            }
            if input.generation == old.generation {
                // An ended generation cannot be resurrected by delayed work.
                if old.state == DerivedState::Ended
                    && !(input.kind == HookKind::SessionStart
                        && input.identity.agent_session_id != old.agent_session_id)
                {
                    return None;
                }
                if (input.identity.agent_session_id != old.agent_session_id
                    || input.adapter_install_id != old.adapter_install_id)
                    && input.kind != HookKind::SessionStart
                {
                    return None;
                }
            } else if input.kind != HookKind::SessionStart {
                return None;
            }
        } else if matches!(input.kind, HookKind::ToolStart | HookKind::ToolEnd) {
            return None;
        }
        let derived = match input.kind {
            HookKind::SessionStart | HookKind::TurnDone | HookKind::Idle => DerivedState::Idle,
            HookKind::PromptSubmitted => DerivedState::Busy,
            HookKind::SessionEnd => DerivedState::Ended,
            HookKind::NeedsApproval => DerivedState::NeedsApproval,
            HookKind::Blocked => DerivedState::Blocked,
            HookKind::Error => DerivedState::Error,
            // Tool completions do not indicate that an agent turn has finished.
            HookKind::ToolStart | HookKind::ToolEnd => old?.state,
        };
        let mut identity = input.identity;
        if let Some(previous) = old.filter(|o| {
            o.agent_session_id == identity.agent_session_id
                && o.identity.adapter_id == identity.adapter_id
        }) {
            if identity.transcript_path.is_none() {
                identity.transcript_path = previous.identity.transcript_path.clone();
            }
            if identity.descriptor_id.is_none() {
                identity.descriptor_id = previous.identity.descriptor_id.clone();
            }
            if identity.resume_argv.is_none() {
                identity.resume_argv = previous.identity.resume_argv.clone();
            }
            if identity.cwd.is_none() {
                identity.cwd = previous.identity.cwd.clone();
            }
        }
        let restore_state = if derived == DerivedState::Ended {
            RestoreState::Ended
        } else if input.kind == HookKind::SessionStart {
            RestoreState::Confirmed
        } else {
            old.filter(|previous| {
                previous.generation == input.generation
                    && previous.agent_session_id == identity.agent_session_id
            })
            .map_or(RestoreState::Unconfirmed, |previous| previous.restore_state)
        };
        let ready = matches!(input.kind, HookKind::Idle | HookKind::TurnDone);
        let mut next = HookStateDto {
            instance_id: self.instance.clone(),
            terminal_id: input.terminal_id,
            generation: input.generation,
            adapter_install_id: input.adapter_install_id,
            agent_session_id: identity.agent_session_id.clone(),
            project_id: input.project_id,
            state: derived,
            ready,
            identity,
            restore_state,
            revision: old.map_or(0, |o| o.revision),
        };
        if old == Some(&next) {
            return None;
        }
        self.revision += 1;
        next.revision = self.revision;
        self.counters.state_upserts += 1;
        if next.state == DerivedState::Ended {
            self.ended.entry(next.terminal_id.clone()).or_insert(now);
        } else {
            self.ended.remove(&next.terminal_id);
        }
        self.states.insert(next.terminal_id.clone(), next.clone());
        self.record(Change {
            at: now,
            project: next.project_id.clone(),
            revision: next.revision,
            state: Some(next.clone()),
            tombstone: None,
        });
        Some(next)
    }
    pub fn set_restore_state(
        &mut self,
        terminal: &str,
        generation: u64,
        lifecycle: RestoreState,
        now: Instant,
    ) -> Option<HookStateDto> {
        let value = self.states.get_mut(terminal)?;
        if value.generation != generation || value.restore_state == lifecycle {
            return None;
        }
        // A delayed spawn/timeout publisher must not regress a concurrently
        // accepted SessionStart. Confirmation is authoritative at ingestion.
        if lifecycle == RestoreState::PendingConfirmation
            && matches!(
                value.restore_state,
                RestoreState::Confirmed | RestoreState::Stale | RestoreState::Ended
            )
        {
            return None;
        }
        if lifecycle == RestoreState::Unconfirmed
            && value.restore_state != RestoreState::PendingConfirmation
        {
            return None;
        }
        if value.restore_state == RestoreState::Ended && lifecycle != RestoreState::Ended {
            return None;
        }
        self.revision += 1;
        value.restore_state = lifecycle;
        value.revision = self.revision;
        let value = value.clone();
        self.counters.state_upserts += 1;
        self.record(Change {
            at: now,
            project: value.project_id.clone(),
            revision: value.revision,
            state: Some(value.clone()),
            tombstone: None,
        });
        Some(value)
    }
    pub fn remove(&mut self, terminal: &str, now: Instant) -> Option<HookStateTombstone> {
        let old = self.states.remove(terminal)?;
        self.ended.remove(terminal);
        self.revision += 1;
        let tombstone = HookStateTombstone {
            instance_id: self.instance.clone(),
            terminal_id: terminal.to_owned(),
            revision: self.revision,
        };
        self.record(Change {
            at: now,
            project: old.project_id,
            revision: self.revision,
            state: None,
            tombstone: Some(tombstone.clone()),
        });
        Some(tombstone)
    }
    pub fn collect(&mut self, now: Instant) {
        self.trim(now);
        let mut ended: Vec<_> = self
            .ended
            .iter()
            .map(|(id, at)| (id.clone(), *at))
            .collect();
        ended.sort_by_key(|(_, at)| *at);
        let excess = ended.len().saturating_sub(ENDED_LIMIT);
        for (index, (id, at)) in ended.into_iter().enumerate() {
            if index < excess || now.duration_since(at) >= ENDED_TTL {
                self.remove(&id, now);
            }
        }
    }
    pub fn sync(
        &mut self,
        request: &HookStateSyncRequest,
        allowed: &dyn Fn(&str) -> bool,
        now: Instant,
    ) -> HookStateSyncResponse {
        self.trim(now);
        let since = request.since_revision;
        let delta = request.instance_id.as_deref() == Some(&self.instance)
            && since.is_some_and(|r| {
                r <= self.revision
                    && (r == self.revision
                        || self
                            .changes
                            .front()
                            .is_some_and(|c| r >= c.revision.saturating_sub(1)))
            });
        if !delta {
            let mut states: Vec<_> = self
                .states
                .values()
                .filter(|s| allowed(&s.project_id))
                .cloned()
                .collect();
            states.sort_by_key(|s| s.revision);
            return HookStateSyncResponse::Snapshot {
                instance_id: self.instance.clone(),
                current_revision: self.revision,
                states,
                tombstones: vec![],
            };
        }
        let mut latest = HashMap::new();
        for change in self
            .changes
            .iter()
            .filter(|c| c.revision > since.unwrap_or(0) && allowed(&c.project))
        {
            let terminal = change
                .state
                .as_ref()
                .map(|s| &s.terminal_id)
                .or_else(|| change.tombstone.as_ref().map(|t| &t.terminal_id))
                .unwrap();
            latest.insert(terminal.clone(), change);
        }
        let mut states: Vec<_> = latest.values().filter_map(|c| c.state.clone()).collect();
        let mut tombstones: Vec<_> = latest
            .values()
            .filter_map(|c| c.tombstone.clone())
            .collect();
        states.sort_by_key(|s| s.revision);
        tombstones.sort_by_key(|s| s.revision);
        HookStateSyncResponse::Delta {
            instance_id: self.instance.clone(),
            current_revision: self.revision,
            states,
            tombstones,
        }
    }
    pub fn latest(&self, terminal: &str) -> Option<HookStateDto> {
        self.states.get(terminal).cloned()
    }
}

type Subscriber = Arc<dyn Fn(&HookStateSyncResponse) + Send + Sync>;
static INSTANCE: OnceLock<String> = OnceLock::new();
static STORE: OnceLock<Mutex<StateStore>> = OnceLock::new();
static SUBSCRIBERS: OnceLock<Mutex<Vec<Subscriber>>> = OnceLock::new();
pub fn instance_id() -> &'static str {
    INSTANCE.get_or_init(|| uuid::Uuid::new_v4().to_string())
}
fn store() -> &'static Mutex<StateStore> {
    STORE.get_or_init(|| Mutex::new(StateStore::new(instance_id().to_owned())))
}
pub fn subscribe(callback: Subscriber) {
    SUBSCRIBERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .push(callback);
}
pub fn sync(
    request: &HookStateSyncRequest,
    allowed: &dyn Fn(&str) -> bool,
) -> HookStateSyncResponse {
    store()
        .lock()
        .unwrap()
        .sync(request, allowed, Instant::now())
}
pub fn latest(terminal: &str) -> Option<HookStateDto> {
    store().lock().unwrap().latest(terminal)
}
/// Filter a pushed envelope using retained tombstone scope. Unknown/expired
/// tombstones fail closed; the subscriber reconciles its authorized snapshot.
pub fn filter_response(
    response: &HookStateSyncResponse,
    allowed: &dyn Fn(&str) -> bool,
) -> HookStateSyncResponse {
    let store = store().lock().unwrap();
    let filter = |states: &Vec<HookStateDto>, tombstones: &Vec<HookStateTombstone>| {
        let states = states
            .iter()
            .filter(|state| allowed(&state.project_id))
            .cloned()
            .collect();
        let tombstones = tombstones
            .iter()
            .filter(|tombstone| {
                store.changes.iter().any(|change| {
                    change.revision == tombstone.revision
                        && change.tombstone.as_ref() == Some(*tombstone)
                        && allowed(&change.project)
                })
            })
            .cloned()
            .collect();
        (states, tombstones)
    };
    match response {
        HookStateSyncResponse::Snapshot {
            instance_id,
            current_revision,
            states,
            tombstones,
        } => {
            let (states, tombstones) = filter(states, tombstones);
            HookStateSyncResponse::Snapshot {
                instance_id: instance_id.clone(),
                current_revision: *current_revision,
                states,
                tombstones,
            }
        }
        HookStateSyncResponse::Delta {
            instance_id,
            current_revision,
            states,
            tombstones,
        } => {
            let (states, tombstones) = filter(states, tombstones);
            HookStateSyncResponse::Delta {
                instance_id: instance_id.clone(),
                current_revision: *current_revision,
                states,
                tombstones,
            }
        }
    }
}
#[derive(Default)]
struct PendingPublications {
    draining: bool,
    queue: VecDeque<HookStateSyncResponse>,
}
static PUBLICATIONS: OnceLock<Mutex<PendingPublications>> = OnceLock::new();

// Enqueue under the store's revision lock. Never hold the queue lock while
// invoking a subscriber: project filtering and chat callbacks re-enter store.
fn enqueue_publication(store: &mut StateStore, message: HookStateSyncResponse) {
    let mut pending = PUBLICATIONS.get_or_init(Default::default).lock().unwrap();
    if pending.queue.len() >= DELTA_LIMIT {
        pending.queue.clear();
        pending.queue.push_back(store.sync(
            &HookStateSyncRequest::default(),
            &|_| true,
            Instant::now(),
        ));
    } else {
        pending.queue.push_back(message);
    }
}
fn drain_publications() {
    let publications = PUBLICATIONS.get_or_init(Default::default);
    {
        let mut pending = publications.lock().unwrap();
        if pending.draining {
            return;
        }
        pending.draining = true;
    }
    loop {
        let message = {
            let mut pending = publications.lock().unwrap();
            match pending.queue.pop_front() {
                Some(message) => message,
                None => {
                    pending.draining = false;
                    return;
                }
            }
        };
        broadcast(message);
    }
}
fn state_delta(value: &HookStateDto) -> HookStateSyncResponse {
    HookStateSyncResponse::Delta {
        instance_id: instance_id().to_owned(),
        current_revision: value.revision,
        states: vec![value.clone()],
        tombstones: vec![],
    }
}

fn broadcast(delta: HookStateSyncResponse) {
    let callbacks = SUBSCRIBERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .clone();
    for callback in callbacks {
        callback(&delta);
    }
}
pub fn upsert(input: StateInput) -> Option<HookStateDto> {
    upsert_with(input, |_, _| {})
}
/// Enrich a changed record before publication. The callback runs only for a
/// visible change and can persist identity edges, never raw tool traffic.
pub fn upsert_with(
    input: StateInput,
    enrich: impl FnOnce(&mut HookStateDto, Option<&HookStateDto>),
) -> Option<HookStateDto> {
    let next = {
        let mut store = store().lock().unwrap();
        let previous = store.latest(&input.terminal_id);
        let mut next = store.upsert(input, Instant::now());
        if let Some(value) = next.as_mut() {
            enrich(value, previous.as_ref());
            store
                .states
                .insert(value.terminal_id.clone(), value.clone());
            if let Some(change) = store.changes.back_mut() {
                change.state = Some(value.clone());
            }
            store.counters.state_broadcasts += 1;
            enqueue_publication(&mut store, state_delta(value));
        }
        next
    };
    drain_publications();
    next
}
pub fn set_restore_state(terminal: &str, generation: u64, lifecycle: RestoreState) -> bool {
    let value = {
        let mut store = store().lock().unwrap();
        let value = store.set_restore_state(terminal, generation, lifecycle, Instant::now());
        if let Some(value) = &value {
            store.counters.state_broadcasts += 1;
            enqueue_publication(&mut store, state_delta(value));
        }
        value
    };
    drain_publications();
    value.is_some()
}
pub fn collect() {
    {
        let mut store = store().lock().unwrap();
        let revision = store.revision;
        store.collect(Instant::now());
        if revision == store.revision {
            return;
        }
        let delta = store.sync(
            &HookStateSyncRequest {
                instance_id: Some(instance_id().to_owned()),
                since_revision: Some(revision),
            },
            &|_| true,
            Instant::now(),
        );
        enqueue_publication(&mut store, delta);
    }
    drain_publications();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(kind: HookKind) -> StateInput {
        StateInput {
            terminal_id: "t".into(),
            generation: 1,
            adapter_install_id: "install".into(),
            project_id: "p".into(),
            identity: HookIdentityDto {
                adapter_id: "pi".to_owned().try_into().unwrap(),
                agent_session_id: "s".into(),
                transcript_path: None,
                cwd: None,
                source: HookIdentitySource::Hook,
                descriptor_id: None,
                resume_argv: None,
            },
            kind,
        }
    }
    #[test]
    fn hundred_tool_events_have_two_changes_zero_logs_zero_disk_writes() {
        let mut store = StateStore::new("instance".into());
        let now = Instant::now();
        store.upsert(input(HookKind::SessionStart), now).unwrap();
        let baseline = store.revision;
        let mut broadcasts = 0;
        broadcasts += usize::from(
            store
                .upsert(input(HookKind::PromptSubmitted), now)
                .is_some(),
        );
        let identity_before_tools = store.latest("t").unwrap().identity;
        for i in 0..100 {
            let mut tool = input(if i % 2 == 0 {
                HookKind::ToolStart
            } else {
                HookKind::ToolEnd
            });
            tool.identity.transcript_path = Some(format!("/tmp/tool-{i}.jsonl"));
            tool.identity.cwd = Some(format!("/tmp/tool-{i}"));
            tool.identity.descriptor_id = Some(format!("tool-{i}"));
            assert!(store.upsert(tool, now).is_none());
        }
        assert_eq!(store.latest("t").unwrap().identity, identity_before_tools);
        broadcasts += usize::from(store.upsert(input(HookKind::TurnDone), now).is_some());
        assert_eq!(broadcasts, 2);
        assert_eq!(store.revision - baseline, 2);
        assert_eq!(store.counters.per_event_logs, 0);
        assert_eq!(store.counters.disk_writes, 0);
        assert!(store.debug.is_empty());
    }
    #[test]
    fn concurrent_publications_and_restore_revisions_are_delivered_in_order() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        let first_id = format!("ordered-first-{}", uuid::Uuid::new_v4());
        let second_id = format!("ordered-second-{}", uuid::Uuid::new_v4());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let (revision_tx, revision_rx) = mpsc::channel();
        let blocked = AtomicBool::new(false);
        let first_filter = first_id.clone();
        let second_filter = second_id.clone();
        subscribe(Arc::new(move |message| {
            let states = match message {
                HookStateSyncResponse::Delta { states, .. }
                | HookStateSyncResponse::Snapshot { states, .. } => states,
            };
            for value in states {
                if value.terminal_id != first_filter && value.terminal_id != second_filter {
                    continue;
                }
                if value.terminal_id == first_filter && !blocked.swap(true, Ordering::SeqCst) {
                    entered_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                }
                // Subscribers may re-enter canonical state without deadlocking.
                assert!(latest(&value.terminal_id).is_some());
                let _ = revision_tx.send(value.revision);
            }
        }));
        let mut first = input(HookKind::SessionStart);
        first.terminal_id = first_id;
        let publishing = std::thread::spawn(move || upsert(first).unwrap());
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let mut second = input(HookKind::SessionStart);
        second.terminal_id = second_id.clone();
        upsert(second).unwrap();
        assert!(set_restore_state(&second_id, 1, RestoreState::Stale));
        release_tx.send(()).unwrap();
        publishing.join().unwrap();
        let revisions: Vec<_> = (0..3)
            .map(|_| revision_rx.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect();
        assert!(
            revisions.windows(2).all(|window| window[0] < window[1]),
            "{revisions:?}"
        );
    }

    #[test]
    fn only_actual_session_start_confirms_and_runtime_cannot_clear_stale() {
        let mut store = StateStore::new("i".into());
        let now = Instant::now();
        assert!(store.upsert(input(HookKind::ToolStart), now).is_none());
        assert_eq!(
            store
                .upsert(input(HookKind::PromptSubmitted), now)
                .unwrap()
                .restore_state,
            RestoreState::Unconfirmed
        );
        assert_eq!(
            store
                .upsert(input(HookKind::SessionStart), now)
                .unwrap()
                .restore_state,
            RestoreState::Confirmed
        );
        assert!(store
            .set_restore_state("t", 2, RestoreState::Stale, now)
            .is_none());
        assert!(store
            .set_restore_state("t", 1, RestoreState::Stale, now)
            .is_some());
        assert!(store
            .set_restore_state("t", 1, RestoreState::Stale, now)
            .is_none());
        assert_eq!(
            store
                .upsert(input(HookKind::PromptSubmitted), now)
                .unwrap()
                .restore_state,
            RestoreState::Stale
        );
        assert!(store.upsert(input(HookKind::ToolEnd), now).is_none());
    }
    #[test]
    fn terminal_generation_and_end_are_monotonic() {
        let mut store = StateStore::new("i".into());
        let now = Instant::now();
        store.upsert(input(HookKind::SessionStart), now);
        store.upsert(input(HookKind::SessionEnd), now).unwrap();
        assert!(store
            .upsert(input(HookKind::PromptSubmitted), now)
            .is_none());
        assert!(store.upsert(input(HookKind::SessionStart), now).is_none());
        let mut next = input(HookKind::SessionStart);
        next.generation = 2;
        store.upsert(next, now).unwrap();
        assert!(store.upsert(input(HookKind::SessionEnd), now).is_none());
    }
    #[test]
    fn scoped_delta_tombstone_expiry_and_instance_replacement() {
        let mut store = StateStore::new("i".into());
        let now = Instant::now();
        store.upsert(input(HookKind::SessionStart), now);
        let request = HookStateSyncRequest {
            instance_id: Some("i".into()),
            since_revision: Some(0),
        };
        match store.sync(&request, &|_| false, now) {
            HookStateSyncResponse::Delta {
                states,
                current_revision,
                ..
            } => {
                assert!(states.is_empty());
                assert_eq!(current_revision, 1);
            }
            _ => panic!(),
        }
        store.remove("t", now);
        match store.sync(&request, &|p| p == "p", now) {
            HookStateSyncResponse::Delta {
                states, tombstones, ..
            } => {
                assert!(states.is_empty());
                assert_eq!(tombstones.len(), 1);
            }
            _ => panic!(),
        }
        assert!(matches!(
            store.sync(
                &request,
                &|_| true,
                now + DELTA_TTL + Duration::from_secs(1)
            ),
            HookStateSyncResponse::Snapshot { .. }
        ));
        assert!(matches!(
            store.sync(
                &HookStateSyncRequest {
                    instance_id: Some("other".into()),
                    since_revision: Some(2)
                },
                &|_| true,
                now
            ),
            HookStateSyncResponse::Snapshot { .. }
        ));
    }
    #[test]
    fn active_records_never_age_out_ended_debug_and_deltas_are_bounded() {
        let mut store = StateStore::new("i".into());
        let now = Instant::now();
        store.set_debug(true);
        store.upsert(input(HookKind::SessionStart), now);
        for i in 0..600 {
            let mut value = input(HookKind::SessionStart);
            value.terminal_id = format!("ended-{i}");
            store.upsert(value.clone(), now);
            value.kind = HookKind::SessionEnd;
            store.upsert(value, now);
        }
        store.collect(now);
        assert_eq!(store.ended.len(), ENDED_LIMIT);
        assert_eq!(store.changes.len(), DELTA_LIMIT);
        assert_eq!(store.debug.len(), DEBUG_LIMIT);
        store.collect(now + ENDED_TTL);
        assert!(store.ended.is_empty());
        assert!(store.latest("t").is_some());
    }
}
