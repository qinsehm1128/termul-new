import type {
  RemoteDeviceAuditEntry,
  RemoteDeviceList,
  RemoteDeviceScope,
  RemotePairingCode
} from '@shared/remote-devices'
import { invoke } from '@tauri-apps/api/core'
import { logFrontendError } from './log-api'
import { remoteAccessHeaders } from './remote-access-credential'
import { isTauriContext } from './tauri-runtime'

/** Raw JSON contract shared by the host commands and same-origin HTTP routes. */
async function request<T>(
  command: string,
  path: string,
  method: 'GET' | 'POST' | 'PATCH' = 'GET',
  args?: Record<string, unknown>,
  body?: Record<string, unknown>
): Promise<T> {
  try {
    let result: T
    if (isTauriContext()) {
      result = await invoke<T>(command, args)
    } else {
      const response = await fetch(`${window.location.origin}${path}`, {
        method,
        headers: remoteAccessHeaders(
          body === undefined ? undefined : { 'content-type': 'application/json' }
        ),
        ...(body === undefined ? {} : { body: JSON.stringify(body) })
      })
      if (!response.ok) throw new Error('remote device request failed')
      result = response.status === 204 ? (undefined as T) : ((await response.json()) as T)
    }
    if (method !== 'GET') {
      void logFrontendError({
        level: 'warn',
        source: 'remote-devices-api',
        message: `${command}: completed`
      })
    }
    return result
  } catch {
    // Never forward credentials, device names, pairing codes or response bodies to logs/UI.
    void logFrontendError({
      source: 'remote-devices-api',
      message: `${command}: failed`
    })
    throw new Error('设备操作失败，请重试')
  }
}

const devicePath = (id: string): string => `/remote/devices/${encodeURIComponent(id)}`

export const remoteDevicesApi = {
  list: (): Promise<RemoteDeviceList> => request('remote_devices_list', '/remote/devices'),
  rename: (id: string, name: string): Promise<void> =>
    request('remote_device_rename', devicePath(id), 'PATCH', { id, name }, { name }),
  setPermissions: (id: string, scopes: RemoteDeviceScope[]): Promise<void> =>
    request('remote_device_set_permissions', devicePath(id), 'PATCH', { id, scopes }, { scopes }),
  grantAll: (id: string): Promise<void> =>
    request('remote_device_grant_all', `${devicePath(id)}/grant-all`, 'POST', { id }),
  disconnect: (id: string): Promise<void> =>
    request('remote_device_disconnect', `${devicePath(id)}/disconnect`, 'POST', { id }),
  revoke: (id: string): Promise<void> =>
    request('remote_device_revoke', `${devicePath(id)}/revoke`, 'POST', { id }),
  /** Force revokes every device and legacy host credential; peers must pair again. */
  disconnectAll: (): Promise<void> =>
    request('remote_devices_disconnect_all', '/remote/devices/disconnect-all', 'POST'),
  audit: (): Promise<RemoteDeviceAuditEntry[]> =>
    request('remote_devices_audit', '/remote/devices/audit'),
  pairingCode: (): Promise<RemotePairingCode> =>
    request('remote_pairing_code', '/remote/pairing-code', 'POST')
}
