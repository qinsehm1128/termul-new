import { REMOTE_DEVICE_SCOPES } from '@shared/remote-devices'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  _resetRemoteAccessCredentialForTests,
  setRemoteAccessCredential
} from './remote-access-credential'
import { remoteDevicesApi } from './remote-devices-api'

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn(), log: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('./tauri-runtime', () => ({ isTauriContext: mocks.isTauri }))
vi.mock('./log-api', () => ({ logFrontendError: mocks.log }))

const device = {
  id: 'phone/one',
  name: '手机',
  platform: 'ios',
  createdAt: 1794210000,
  lastSeen: 1794210020,
  scopes: ['view'],
  revokedAt: null,
  legacy: false,
  projectId: 'p1',
  connectionCount: 2
}
const cases = [
  {
    command: 'remote_devices_list',
    path: '/remote/devices',
    method: 'GET',
    run: () => remoteDevicesApi.list(),
    result: { devices: [device], revision: 7 }
  },
  {
    command: 'remote_device_rename',
    path: '/remote/devices/phone%2Fone',
    method: 'PATCH',
    args: { id: 'phone/one', name: '新名称' },
    body: { name: '新名称' },
    run: () => remoteDevicesApi.rename('phone/one', '新名称'),
    result: null
  },
  {
    command: 'remote_device_set_permissions',
    path: '/remote/devices/phone%2Fone',
    method: 'PATCH',
    args: { id: 'phone/one', scopes: [...REMOTE_DEVICE_SCOPES] },
    body: { scopes: [...REMOTE_DEVICE_SCOPES] },
    run: () => remoteDevicesApi.setPermissions('phone/one', [...REMOTE_DEVICE_SCOPES]),
    result: null
  },
  {
    command: 'remote_device_grant_all',
    path: '/remote/devices/phone%2Fone/grant-all',
    method: 'POST',
    args: { id: 'phone/one' },
    run: () => remoteDevicesApi.grantAll('phone/one'),
    result: null
  },
  {
    command: 'remote_device_disconnect',
    path: '/remote/devices/phone%2Fone/disconnect',
    method: 'POST',
    args: { id: 'phone/one' },
    run: () => remoteDevicesApi.disconnect('phone/one'),
    result: null
  },
  {
    command: 'remote_device_revoke',
    path: '/remote/devices/phone%2Fone/revoke',
    method: 'POST',
    args: { id: 'phone/one' },
    run: () => remoteDevicesApi.revoke('phone/one'),
    result: null
  },
  {
    command: 'remote_devices_disconnect_all',
    path: '/remote/devices/disconnect-all',
    method: 'POST',
    run: () => remoteDevicesApi.disconnectAll(),
    result: null
  },
  {
    command: 'remote_devices_audit',
    path: '/remote/devices/audit',
    method: 'GET',
    run: () => remoteDevicesApi.audit(),
    result: [
      { at: 1794210020, event: 'scope_denied', deviceId: 'phone/one', capability: 'git.write' }
    ]
  },
  {
    command: 'remote_pairing_code',
    path: '/remote/pairing-code',
    method: 'POST',
    run: () => remoteDevicesApi.pairingCode(),
    result: { code: 'ONE_TIME_SECRET', expiresAt: 1794210080 }
  }
]

describe('remote device facade parity', () => {
  const fetchMock = vi.fn()
  beforeEach(() => {
    vi.clearAllMocks()
    _resetRemoteAccessCredentialForTests()
    setRemoteAccessCredential('DEVICE_BEARER')
    vi.stubGlobal('fetch', fetchMock)
    mocks.log.mockResolvedValue(undefined)
  })
  for (const entry of cases) {
    it(`${entry.command}: Tauri and HTTP preserve the same raw JSON contract`, async () => {
      mocks.isTauri.mockReturnValue(true)
      mocks.invoke.mockResolvedValue(entry.result)
      const desktop = await entry.run()
      expect(mocks.invoke).toHaveBeenCalledWith(
        entry.command,
        'args' in entry ? entry.args : undefined
      )
      expect(fetchMock).not.toHaveBeenCalled()
      mocks.invoke.mockClear()
      mocks.isTauri.mockReturnValue(false)
      fetchMock.mockResolvedValue(new Response(JSON.stringify(entry.result), { status: 200 }))
      const web = await entry.run()
      expect(web).toEqual(desktop)
      expect(web).toEqual(entry.result)
      expect(mocks.invoke).not.toHaveBeenCalled()
      expect(fetchMock).toHaveBeenCalledWith(`${window.location.origin}${entry.path}`, {
        method: entry.method,
        headers: new Headers({
          authorization: 'Bearer DEVICE_BEARER',
          ...('body' in entry ? { 'content-type': 'application/json' } : {})
        }),
        ...('body' in entry ? { body: JSON.stringify(entry.body) } : {})
      })
      expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('DEVICE_BEARER')
      expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('ONE_TIME_SECRET')
      expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('新名称')
    })
  }
  it('never grants manage as a device scope', () => {
    expect(REMOTE_DEVICE_SCOPES).toHaveLength(8)
    expect(REMOTE_DEVICE_SCOPES).not.toContain('manage')
  })
  it.each([true, false])('redacts errors and logs failures (Tauri=%s)', async (tauri) => {
    mocks.isTauri.mockReturnValue(tauri)
    mocks.invoke.mockRejectedValue(new Error('secret-host-token'))
    fetchMock.mockResolvedValue(new Response('secret-host-token', { status: 403 }))
    await expect(remoteDevicesApi.pairingCode()).rejects.toThrow('设备操作失败，请重试')
    expect(mocks.log).toHaveBeenCalledWith({
      source: 'remote-devices-api',
      message: 'remote_pairing_code: failed'
    })
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('secret-host-token')
  })
  it('handles empty successful HTTP mutation responses', async () => {
    mocks.isTauri.mockReturnValue(false)
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }))
    await expect(remoteDevicesApi.disconnectAll()).resolves.toBeUndefined()
  })
  it('logs network and malformed JSON failures without leaking payloads', async () => {
    mocks.isTauri.mockReturnValue(false)
    fetchMock.mockRejectedValueOnce(new Error('secret-host-token'))
    await expect(remoteDevicesApi.list()).rejects.toThrow('设备操作失败')
    fetchMock.mockResolvedValueOnce(new Response('not-json', { status: 200 }))
    await expect(remoteDevicesApi.list()).rejects.toThrow('设备操作失败')
    expect(mocks.log).toHaveBeenCalledTimes(2)
  })
})
