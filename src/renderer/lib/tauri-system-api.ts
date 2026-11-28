import type { IpcResult, SystemApi } from '@shared/types/ipc.types'
import { type InvokeArgs, invoke } from '@tauri-apps/api/core'

/**
 * IPC Event names
 *
 * NOTE: The POWER_RESUME event is NOT emitted by the backend.
 * This file uses a heartbeat-based fallback approach instead.
 *
 * See decision document: docs/decisions/keyboard-system-event-mode-final.md
 *
 * In Electron, power resume detection uses 'system:power-resume' event,
 * but the Electron backend ALSO does not emit this event (it's a stub).
 * Therefore, using a heartbeat fallback provides actual parity (or better).
 *
 * The heartbeat approach works by:
 * 1. Setting a recurring interval timer (30 seconds)
 * 2. Tracking the last heartbeat timestamp
 * 3. If more than 2 intervals have passed, system was likely asleep
 * 4. Triggering the resume callback when sleep is detected
 */
const _IPC_EVENTS = {
  POWER_RESUME: 'system://power-resume' // Not emitted - using heartbeat fallback instead
} as const

/**
 * IPC Command names
 * Must match Rust command names in src-tauri/src/lib.rs
 */
const IPC_COMMANDS = {
  GET_HOME_DIRECTORY: 'get_home_directory'
} as const

/**
 * Wrap invoke() calls in IpcResult<T> pattern with try/catch
 */
async function invokeIpc<T>(command: string, args?: InvokeArgs): Promise<IpcResult<T>> {
  try {
    const data = await invoke<T>(command, args)
    return { success: true, data }
  } catch (error) {
    return {
      success: false,
      error: error instanceof Error ? error.message : String(error),
      code: 'UNKNOWN_ERROR'
    }
  }
}

/**
 * Heartbeat interval for power resume detection.
 *
 * This is the CHOSEN approach for detecting system sleep/resume, not a workaround.
 * The Electron implementation also lacks a proper backend emitter for power-resume,
 * so this heartbeat method provides equivalent (or better) functionality.
 *
 * Interval: 30 seconds
 * Detection threshold: 2x interval (60 seconds) = system was asleep
 */
const HEARTBEAT_INTERVAL_MS = 30_000

/**
 * One shared heartbeat, reference counted by subscriber.
 *
 * Every mounted terminal subscribes (`ConnectedTerminal`), so a timer per
 * subscription is both waste and a leak: the module used to hold a single
 * `heartbeatTimer` handle that each new subscriber overwrote, which left every
 * earlier interval running with no way to clear it. A leaked interval is not
 * merely a 30s wake-up — its callback closes over the unmounted component's
 * refs, so it retains that terminal's xterm instance, buffer and DOM for the
 * lifetime of the renderer.
 */
const resumeListeners = new Set<() => void>()
let heartbeatTimer: ReturnType<typeof setInterval> | null = null
let lastHeartbeat = Date.now()

function startHeartbeatIfNeeded(): void {
  if (heartbeatTimer !== null) return
  lastHeartbeat = Date.now()
  heartbeatTimer = setInterval(() => {
    const now = Date.now()
    const elapsed = now - lastHeartbeat

    if (elapsed > HEARTBEAT_INTERVAL_MS * 2) {
      // System likely resumed from sleep: more than 2 intervals have passed
      // without the timer running. Snapshot first — a listener may unsubscribe.
      for (const listener of [...resumeListeners]) {
        try {
          listener()
        } catch {
          // A resume handler failing must not stop the others or the timer.
        }
      }
    }
    lastHeartbeat = now
  }, HEARTBEAT_INTERVAL_MS)
}

function stopHeartbeatIfIdle(): void {
  if (resumeListeners.size > 0 || heartbeatTimer === null) return
  clearInterval(heartbeatTimer)
  heartbeatTimer = null
}

function subscribePowerResume(callback: () => void): () => void {
  // Heartbeat-based power resume detection (chosen approach)
  // See: docs/decisions/keyboard-system-event-mode-final.md
  resumeListeners.add(callback)
  startHeartbeatIfNeeded()

  let subscribed = true
  return () => {
    if (!subscribed) return
    subscribed = false
    resumeListeners.delete(callback)
    stopHeartbeatIfIdle()
  }
}

/**
 * Create a SystemApi implementation using Tauri IPC
 *
 * FALLBACK MODE for power resume detection:
 *
 * The onPowerResume implementation uses a heartbeat-based approach rather than
 * backend event emitters. This is the intentional design decision, documented in:
 * docs/decisions/keyboard-system-event-mode-final.md
 *
 * Rationale:
 * - OS-specific power monitoring in Rust is complex (windows-rs, NSWorkspace, systemd)
 * - Electron's power-resume is also a stub (no actual backend emitter)
 * - Heartbeat approach is cross-platform and reliable
 * - Estimated effort for proper backend events: 2-3 days (not worth the value)
 */
export function createTauriSystemApi(): SystemApi {
  return {
    async getHomeDirectory(): Promise<IpcResult<string>> {
      return invokeIpc<string>(IPC_COMMANDS.GET_HOME_DIRECTORY)
    },

    onPowerResume(callback: () => void): () => void {
      // One refcounted heartbeat shared by every subscriber — a timer per
      // terminal leaks one interval (and the terminal it closes over) per
      // mount. See `subscribePowerResume`.
      return subscribePowerResume(callback)
    }
  }
}

/**
 * Direct export singleton for convenience (matches api-bridge pattern)
 *
 * FALLBACK MODE for power resume detection - see documentation above.
 *
 * NOTE: getPlatform was removed - the corresponding Rust command does not exist.
 * Use tauri-plugin-os's platform() directly if needed.
 */
export const tauriSystemApi = {
  async getHomeDirectory(): Promise<IpcResult<string>> {
    return invokeIpc<string>(IPC_COMMANDS.GET_HOME_DIRECTORY)
  },

  onPowerResume(callback: () => void): () => void {
    // Shared refcounted heartbeat — see `subscribePowerResume`.
    return subscribePowerResume(callback)
  }
}

/**
 * @internal Testing only - reset heartbeat state between tests
 */
export function _resetSystemHeartbeatForTesting(): void {
  if (heartbeatTimer) {
    clearInterval(heartbeatTimer)
    heartbeatTimer = null
  }
  resumeListeners.clear()
  lastHeartbeat = Date.now()
}
