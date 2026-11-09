//! Local consent, revocation and activity state for the remote desktop routes.
//! Remote transports may inspect consent, but only local commands may change it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tokio::sync::broadcast;

use super::tunnel::config::TunnelConfigStore;
use crate::screen;

pub const STATE_EVENT: &str = "remote-desktop://state";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPermissions {
    pub screen_recording: bool,
    pub accessibility: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSettings {
    pub supported: bool,
    pub view_enabled: bool,
    pub control_enabled: bool,
    pub permissions: DesktopPermissions,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopState {
    pub viewers: usize,
    pub controllers: usize,
    pub display: Option<String>,
    pub control_enabled: bool,
}

struct State {
    view_enabled: bool,
    control_enabled: bool,
    viewers: BTreeMap<u64, String>,
    controllers: BTreeSet<u64>,
}

pub struct DesktopService {
    store: Option<Arc<TunnelConfigStore>>,
    state: Mutex<State>,
    stop: broadcast::Sender<()>,
    sink: Box<dyn Fn(DesktopState) + Send + Sync>,
    native: bool,
}

static GLOBAL: OnceLock<Arc<DesktopService>> = OnceLock::new();

impl DesktopService {
    /// Install before starting HTTP/WS listeners; restore persisted local consent.
    pub fn initialize(
        store: Arc<TunnelConfigStore>,
        sink: impl Fn(DesktopState) + Send + Sync + 'static,
    ) -> Result<Arc<Self>, String> {
        let config = store.load()?;
        let service = Self::new(Some(store), config.desktop_view_enabled, false, true, sink);
        screen::kill_switch();
        if config.desktop_view_enabled && config.desktop_control_enabled {
            match screen::rearm() {
                Ok(()) => {
                    service
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .control_enabled = true;
                    audit("desktop_control_enable", "OK");
                }
                Err(error) => {
                    service.persist(config.desktop_view_enabled, false)?;
                    audit("desktop_control_enable", error.code());
                }
            }
        }
        GLOBAL
            .set(service.clone())
            .map_err(|_| "desktop service already initialized".to_string())?;
        service.emit_state();
        Self::start_replay(&service);
        Ok(service)
    }

    fn start_replay(service: &Arc<Self>) {
        let weak = Arc::downgrade(service);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let Some(service) = weak.upgrade() else { break };
                // Include zero snapshots to clear a renderer that missed a stop.
                service.emit_state();
            }
        });
    }

    pub fn global() -> Arc<Self> {
        GLOBAL
            .get()
            .expect("DesktopService must be initialized before remote listeners")
            .clone()
    }

    fn new(
        store: Option<Arc<TunnelConfigStore>>,
        view_enabled: bool,
        control_enabled: bool,
        native: bool,
        sink: impl Fn(DesktopState) + Send + Sync + 'static,
    ) -> Arc<Self> {
        let (stop, _) = broadcast::channel(16);
        Arc::new(Self {
            store,
            state: Mutex::new(State {
                view_enabled,
                control_enabled,
                viewers: BTreeMap::new(),
                controllers: BTreeSet::new(),
            }),
            stop,
            sink: Box::new(sink),
            native,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_tests(view_enabled: bool, control_enabled: bool) -> Arc<Self> {
        Self::new(None, view_enabled, control_enabled, false, |_| {})
    }

    fn snapshot(&self, state: &State) -> DesktopSettings {
        DesktopSettings {
            supported: screen::supported(),
            view_enabled: state.view_enabled,
            control_enabled: state.control_enabled,
            permissions: if self.native {
                permissions()
            } else {
                DesktopPermissions {
                    screen_recording: false,
                    accessibility: false,
                }
            },
        }
    }

    pub fn settings(&self) -> DesktopSettings {
        self.snapshot(&self.state.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn view_enabled(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .view_enabled
    }

    pub fn control_enabled(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .control_enabled
    }

    pub fn subscribe_stop(&self) -> broadcast::Receiver<()> {
        self.stop.subscribe()
    }

    fn persist(&self, view: bool, control: bool) -> Result<(), String> {
        match &self.store {
            Some(store) => store.set_desktop_settings(view, control),
            None => Ok(()),
        }
    }

    fn kill(&self, state: &mut State, stop_streams: bool) {
        state.control_enabled = false;
        state.controllers.clear();
        if self.native {
            screen::kill_switch();
        }
        if stop_streams {
            state.viewers.clear();
            let _ = self.stop.send(());
        }
    }

    pub fn set_settings(
        &self,
        view_enabled: bool,
        control_enabled: bool,
    ) -> Result<DesktopSettings, String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Input cannot remain armed with screen sharing disabled.
        let control_enabled = view_enabled && control_enabled;
        let was_control = state.control_enabled;
        state.view_enabled = view_enabled;
        if !control_enabled {
            self.kill(&mut state, !view_enabled);
        } else if !was_control && self.native {
            if let Err(error) = screen::rearm() {
                self.emit(&state);
                audit("desktop_control_enable", error.code());
                return Err(error.to_string());
            }
        }
        if let Err(error) = self.persist(view_enabled, control_enabled) {
            // Failed writes must never leave newly-enabled input live.
            self.kill(&mut state, !view_enabled);
            self.emit(&state);
            audit(
                if control_enabled {
                    "desktop_control_enable"
                } else {
                    "desktop_control_disable"
                },
                "PERSIST_FAILED",
            );
            return Err(error);
        }
        state.control_enabled = control_enabled;
        self.emit(&state);
        audit(
            if control_enabled {
                "desktop_control_enable"
            } else {
                "desktop_control_disable"
            },
            "OK",
        );
        Ok(self.snapshot(&state))
    }

    pub fn emergency_stop(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.kill(&mut state, true);
        let result = self.persist(state.view_enabled, false);
        self.emit(&state);
        audit(
            "desktop_emergency_stop",
            if result.is_ok() {
                "OK"
            } else {
                "PERSIST_FAILED"
            },
        );
        result
    }

    pub fn emit_state(&self) {
        self.emit(&self.state.lock().unwrap_or_else(|e| e.into_inner()));
    }

    fn emit(&self, state: &State) {
        (self.sink)(DesktopState {
            viewers: state.viewers.len(),
            controllers: state.controllers.len(),
            display: state.viewers.values().next().cloned(),
            control_enabled: state.control_enabled,
        });
    }

    pub fn viewer_started(&self, id: u64, display: String) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.view_enabled {
            state.viewers.insert(id, display);
            self.emit(&state);
        }
    }

    pub fn viewer_stopped(&self, id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.viewers.remove(&id);
        state.controllers.remove(&id);
        self.emit(&state);
    }

    pub fn set_viewer_display(&self, id: u64, display: String) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(current) = state.viewers.get_mut(&id) {
            *current = display;
            self.emit(&state);
        }
    }

    pub fn controller_active(&self, id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.control_enabled && state.viewers.contains_key(&id) {
            state.controllers.insert(id);
            self.emit(&state);
        }
    }
}

fn audit(operation: &str, code: &str) {
    log::info!(target: "se_manager::remote::desktop", "operation={operation} device_id=local display=none stable_code={code}");
}

fn permissions() -> DesktopPermissions {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrusted() -> u8;
            fn CGPreflightScreenCaptureAccess() -> u8;
        }
        // Passive TCC queries only; opening settings must not prompt.
        unsafe {
            DesktopPermissions {
                screen_recording: CGPreflightScreenCaptureAccess() != 0,
                accessibility: AXIsProcessTrusted() != 0,
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        DesktopPermissions {
            screen_recording: screen::supported(),
            accessibility: screen::supported(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed() -> (Arc<DesktopService>, Arc<Mutex<Vec<DesktopState>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let output = events.clone();
        let service = DesktopService::new(None, true, false, false, move |state| {
            output.lock().unwrap().push(state);
        });
        (service, events)
    }

    #[tokio::test]
    async fn periodic_replay_restores_current_state_without_new_activity() {
        let (service, events) = observed();
        service.viewer_started(7, "Display 1".into());
        let expected = events.lock().unwrap().last().unwrap().clone();
        events.lock().unwrap().clear();
        DesktopService::start_replay(&service);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if !events.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(events.lock().unwrap().last(), Some(&expected));
        service.viewer_stopped(7);
        events.lock().unwrap().clear();
        service.emit_state();
        assert_eq!(events.lock().unwrap().last().unwrap().viewers, 0);
    }

    #[test]
    fn settings_serialize_camel_case_with_boolean_permissions() {
        let service = DesktopService::for_tests(true, false);
        let value = serde_json::to_value(service.settings()).unwrap();
        assert_eq!(value["viewEnabled"], true);
        assert_eq!(value["controlEnabled"], false);
        assert!(value["supported"].is_boolean());
        assert!(value["permissions"]["screenRecording"].is_boolean());
        assert!(value["permissions"]["accessibility"].is_boolean());
    }

    #[test]
    fn emergency_stop_notifies_all_subscribers_and_resets_activity() {
        let (service, events) = observed();
        let mut first = service.subscribe_stop();
        let mut second = service.subscribe_stop();
        service.set_settings(true, true).unwrap();
        service.viewer_started(1, "Display 1".into());
        service.controller_active(1);
        service.emergency_stop().unwrap();
        assert_eq!(first.try_recv(), Ok(()));
        assert_eq!(second.try_recv(), Ok(()));
        assert!(service.view_enabled());
        assert!(!service.control_enabled());
        let state = events.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            state,
            DesktopState {
                viewers: 0,
                controllers: 0,
                display: None,
                control_enabled: false
            }
        );
    }

    #[test]
    fn disable_view_stops_streams_and_control() {
        let service = DesktopService::for_tests(true, true);
        let mut stop = service.subscribe_stop();
        let settings = service.set_settings(false, true).unwrap();
        assert!(!settings.view_enabled);
        assert!(!settings.control_enabled);
        assert_eq!(stop.try_recv(), Ok(()));
    }

    #[test]
    fn disable_control_preserves_viewers_and_reenable_works() {
        let (service, events) = observed();
        service.set_settings(true, true).unwrap();
        service.viewer_started(1, "Display".into());
        service.controller_active(1);
        service.set_settings(true, false).unwrap();
        assert_eq!(events.lock().unwrap().last().unwrap().viewers, 1);
        assert_eq!(events.lock().unwrap().last().unwrap().controllers, 0);
        service.controller_active(1);
        assert_eq!(events.lock().unwrap().last().unwrap().controllers, 0);
        service.set_settings(true, true).unwrap();
        service.controller_active(1);
        assert_eq!(events.lock().unwrap().last().unwrap().controllers, 1);
    }

    #[test]
    fn viewer_lifecycle_is_idempotent_and_tracks_display() {
        let (service, events) = observed();
        service.set_settings(true, true).unwrap();
        service.viewer_started(5, "First".into());
        service.viewer_started(5, "Replacement".into());
        service.set_viewer_display(5, "Second".into());
        service.controller_active(5);
        service.controller_active(5);
        let state = events.lock().unwrap().last().unwrap().clone();
        assert_eq!((state.viewers, state.controllers), (1, 1));
        assert_eq!(state.display.as_deref(), Some("Second"));
        service.viewer_stopped(5);
        service.viewer_stopped(5);
        let state = events.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            (state.viewers, state.controllers, state.display),
            (0, 0, None)
        );
    }

    #[test]
    fn stopped_view_does_not_admit_viewer_or_controller() {
        let (service, events) = observed();
        service.set_settings(false, false).unwrap();
        service.viewer_started(5, "Display".into());
        service.controller_active(5);
        assert_eq!(events.lock().unwrap().last().unwrap().viewers, 0);
        assert_eq!(events.lock().unwrap().last().unwrap().controllers, 0);
    }

    #[test]
    fn service_updates_and_emergency_stop_persist_in_existing_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(TunnelConfigStore::new(dir.path().to_path_buf()));
        let service = DesktopService::new(Some(store.clone()), true, false, false, |_| {});
        service.set_settings(true, true).unwrap();
        assert!(store.load().unwrap().desktop_control_enabled);
        service.emergency_stop().unwrap();
        let config = store.load().unwrap();
        assert!(config.desktop_view_enabled);
        assert!(!config.desktop_control_enabled);
        service.set_settings(false, false).unwrap();
        assert!(!store.load().unwrap().desktop_view_enabled);
    }

    #[test]
    fn persistence_failure_still_revokes_and_broadcasts_stop() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("not-a-directory");
        std::fs::write(&blocked, "blocked").unwrap();
        let store = Arc::new(TunnelConfigStore::new(blocked));
        let service = DesktopService::new(Some(store), true, true, false, |_| {});
        let mut stop = service.subscribe_stop();
        assert!(service.emergency_stop().is_err());
        assert!(!service.control_enabled());
        assert_eq!(stop.try_recv(), Ok(()));
        assert!(service.set_settings(true, true).is_err());
        assert!(!service.control_enabled());
    }
}

#[tauri::command]
pub fn get_remote_desktop_settings() -> DesktopSettings {
    DesktopService::global().settings()
}

#[tauri::command]
pub fn set_remote_desktop_settings(
    view_enabled: bool,
    control_enabled: bool,
) -> Result<DesktopSettings, String> {
    DesktopService::global().set_settings(view_enabled, control_enabled)
}

#[tauri::command]
pub fn remote_desktop_emergency_stop() -> Result<(), String> {
    DesktopService::global().emergency_stop()
}
