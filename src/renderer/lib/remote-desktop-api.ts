import { invoke } from '@tauri-apps/api/core'
import { logFrontendError } from './log-api'
import { listen, type UnlistenFn } from './tauri-event'
import { isTauriContext } from './tauri-runtime'

export interface RemoteDesktopSettings {
  supported: boolean
  viewEnabled: boolean
  controlEnabled: boolean
  permissions: { screenRecording: boolean; accessibility: boolean }
}

export interface RemoteDesktopSettingsUpdate {
  viewEnabled: boolean
  controlEnabled: boolean
}

export interface RemoteDesktopState {
  viewers: number
  controllers: number
  display: string | null
  controlEnabled: boolean
}

export const REMOTE_DESKTOP_STATE_EVENT = 'remote-desktop://state'

function unsupportedSettings(): RemoteDesktopSettings {
  return {
    supported: false,
    viewEnabled: false,
    controlEnabled: false,
    permissions: { screenRecording: false, accessibility: false }
  }
}

async function operation<T>(name: string, run: () => Promise<T>): Promise<T> {
  try {
    return await run()
  } catch {
    // Never include response bodies, display names, or host credentials in logs/UI.
    void logFrontendError({ source: 'remote-desktop-api', message: `${name}: failed` })
    throw new Error('远程桌面操作失败，请重试')
  }
}

async function webRequest<T>(
  path: string,
  method: 'GET' | 'PUT' | 'POST',
  body?: unknown
): Promise<T> {
  const response = await fetch(`${window.location.origin}${path}`, {
    method,
    ...(body === undefined
      ? {}
      : {
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(body)
        })
  })
  if (!response.ok) throw new Error('remote desktop request failed')
  return response.status === 204 ? (undefined as T) : ((await response.json()) as T)
}

/** Explicit transport parity for the local host routes; not a remote browser capability. */
export const webRemoteDesktopApi = {
  getSettings: (): Promise<RemoteDesktopSettings> =>
    operation('get_remote_desktop_settings', () =>
      webRequest('/api/remote-desktop/settings', 'GET')
    ),
  setSettings: (settings: RemoteDesktopSettingsUpdate): Promise<RemoteDesktopSettings> =>
    operation('set_remote_desktop_settings', () =>
      webRequest('/api/remote-desktop/settings', 'PUT', settings)
    ),
  emergencyStop: (): Promise<void> =>
    operation('remote_desktop_emergency_stop', () => webRequest('/api/remote-desktop/stop', 'POST'))
}

export const tauriRemoteDesktopApi = {
  getSettings: (): Promise<RemoteDesktopSettings> =>
    operation('get_remote_desktop_settings', () => invoke('get_remote_desktop_settings')),
  setSettings: (settings: RemoteDesktopSettingsUpdate): Promise<RemoteDesktopSettings> =>
    operation('set_remote_desktop_settings', () =>
      invoke('set_remote_desktop_settings', { ...settings })
    ),
  emergencyStop: (): Promise<void> =>
    operation('remote_desktop_emergency_stop', () => invoke('remote_desktop_emergency_stop'))
}

function requireLocalHost(): void {
  if (!isTauriContext()) throw new Error('请在桌面客户端设置远程桌面')
}

/** Host consent is local-only. A browser must never modify the computer it is viewing. */
export const remoteDesktopApi = {
  getSettings: async (): Promise<RemoteDesktopSettings> => {
    if (!isTauriContext()) return unsupportedSettings()
    return tauriRemoteDesktopApi.getSettings()
  },
  setSettings: async (settings: RemoteDesktopSettingsUpdate): Promise<RemoteDesktopSettings> => {
    requireLocalHost()
    return tauriRemoteDesktopApi.setSettings(settings)
  },
  emergencyStop: async (): Promise<void> => {
    requireLocalHost()
    return tauriRemoteDesktopApi.emergencyStop()
  },
  subscribeState: async (onState: (state: RemoteDesktopState) => void): Promise<UnlistenFn> => {
    if (!isTauriContext()) return () => {}
    return operation('remote_desktop_subscribe_state', () =>
      listen<RemoteDesktopState>(REMOTE_DESKTOP_STATE_EVENT, (event) => onState(event.payload))
    )
  }
}
