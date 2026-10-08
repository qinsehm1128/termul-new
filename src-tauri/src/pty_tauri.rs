//! Tauri adapters for the transport-neutral PTY runtime: terminal output goes
//! to the renderer's IPC channel and terminal events to the app emitter.

use std::sync::Arc;

use se_pty::trackers::{DesktopEventSink, TerminalEventHub};
use se_pty::OutputSink;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Emitter};

struct ChannelOutput(Channel<Response>);

impl OutputSink for ChannelOutput {
    fn send(&self, data: Vec<u8>) -> Result<(), String> {
        self.0
            .send(Response::new(data))
            .map_err(|error| error.to_string())
    }
}

/// Stream a terminal's output into the renderer's IPC channel.
pub(crate) fn channel_output(channel: Channel<Response>) -> Arc<dyn OutputSink> {
    Arc::new(ChannelOutput(channel))
}

struct AppEmitter(AppHandle);

impl DesktopEventSink for AppEmitter {
    fn emit(&self, event: &str, payload: serde_json::Value) -> Result<(), String> {
        self.0
            .emit(event, payload)
            .map_err(|error| error.to_string())
    }
}

/// A terminal event hub that also emits each event to the desktop renderer.
pub(crate) fn desktop_terminal_events(app: AppHandle) -> TerminalEventHub {
    TerminalEventHub::desktop(Arc::new(AppEmitter(app)))
}
