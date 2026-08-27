import type { IpcResult, RemoteStatus } from '@shared/types/ipc.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockInvoke } = vi.hoisted(() => ({
  mockInvoke: vi.fn()
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: mockInvoke
}))

import { remoteServerApi, syncProjects, tunnelConfigApi } from './tauri-remote-api'

describe('remoteServerApi', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('start() forwards the Rust IpcResult unchanged (no double-wrap)', async () => {
    const status: RemoteStatus = {
      running: true,
      url: 'http://127.0.0.1:5180',
      port: 5180,
      bindMode: 'localhost',
      bindHost: '127.0.0.1'
    }
    const ipc: IpcResult<RemoteStatus> = { success: true, data: status }
    mockInvoke.mockResolvedValueOnce(ipc)

    const result = await remoteServerApi.start({ bindMode: 'localhost' })

    expect(mockInvoke).toHaveBeenCalledWith('remote_server_start', { bindMode: 'localhost' })
    expect(result).toEqual(ipc)
  })

  it('stop() calls the remote_server_stop command', async () => {
    const ipc: IpcResult<RemoteStatus> = {
      success: true,
      data: { running: false, url: null, port: null, bindMode: null, bindHost: null }
    }
    mockInvoke.mockResolvedValueOnce(ipc)

    const result = await remoteServerApi.stop()

    expect(mockInvoke).toHaveBeenCalledWith('remote_server_stop', undefined)
    expect(result.success).toBe(true)
  })

  it('status() calls the remote_server_status command', async () => {
    const ipc: IpcResult<RemoteStatus> = {
      success: true,
      data: { running: false, url: null, port: null, bindMode: null, bindHost: null }
    }
    mockInvoke.mockResolvedValueOnce(ipc)

    await remoteServerApi.status()

    expect(mockInvoke).toHaveBeenCalledWith('remote_server_status', undefined)
  })

  it('wraps a thrown invoke error into a failed IpcResult', async () => {
    mockInvoke.mockRejectedValueOnce(new Error('backend unavailable'))

    const result = await remoteServerApi.start()

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.error).toBe('backend unavailable')
      expect(result.code).toBe('INVOKE_ERROR')
    }
  })

  it('propagates a Rust-side failure IpcResult', async () => {
    const ipc: IpcResult<RemoteStatus> = {
      success: false,
      error: 'Remote server is already running',
      code: 'REMOTE_START_FAILED'
    }
    mockInvoke.mockResolvedValueOnce(ipc)

    const result = await remoteServerApi.start()

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.code).toBe('REMOTE_START_FAILED')
    }
  })

  it('intent() and rotateCredential() forward the new remote commands', async () => {
    mockInvoke.mockResolvedValueOnce({
      success: true,
      data: { wanted: true, publishMode: 'lan' }
    })
    await remoteServerApi.intent()
    expect(mockInvoke).toHaveBeenCalledWith('remote_access_intent_get', undefined)

    mockInvoke.mockResolvedValueOnce({
      success: true,
      data: { wanted: true, publishMode: 'tunnel' }
    })
    await remoteServerApi.setIntent({ publishMode: 'tunnel' })
    expect(mockInvoke).toHaveBeenCalledWith('remote_access_intent_set', {
      update: { publishMode: 'tunnel' }
    })

    mockInvoke.mockResolvedValueOnce({
      success: true,
      data: { running: false, url: null, port: null, bindMode: null, bindHost: null }
    })
    await remoteServerApi.rotateCredential()
    expect(mockInvoke).toHaveBeenCalledWith('remote_server_rotate_credential', undefined)
  })

  it('tunnelConfigApi.get forwards tunnel_config_get', async () => {
    const ipc: IpcResult<{ provider: 'cloudflareQuick' }> = {
      success: true,
      data: { provider: 'cloudflareQuick' }
    }
    mockInvoke.mockResolvedValueOnce(ipc)
    const result = await tunnelConfigApi.get()
    expect(mockInvoke).toHaveBeenCalledWith('tunnel_config_get', undefined)
    expect(result).toEqual(ipc)
  })

  it('tunnelConfigApi.set forwards the update payload', async () => {
    mockInvoke.mockResolvedValueOnce({ success: true, data: { provider: 'frp' } })
    await tunnelConfigApi.set({ provider: 'frp', frpServerAddr: '1.2.3.4' })
    expect(mockInvoke).toHaveBeenCalledWith('tunnel_config_set', {
      update: { provider: 'frp', frpServerAddr: '1.2.3.4' }
    })
  })

  it('syncProjects transports project groups in the desktop sync payload', async () => {
    const ipc: IpcResult<void> = { success: true, data: undefined }
    mockInvoke.mockResolvedValueOnce(ipc)
    const projects = [
      {
        id: 'p-1',
        name: 'Project',
        color: 'blue',
        path: '/tmp/project',
        isArchived: false,
        isDefault: true
      }
    ]
    const groups = [
      {
        id: 'g-1',
        name: 'Favorites',
        projectIds: ['p-1'],
        color: 'purple',
        preferredProjectId: 'p-1'
      }
    ]

    const result = await syncProjects(projects, 'p-1', groups)

    expect(mockInvoke).toHaveBeenCalledWith('remote_sync_projects', {
      payload: { projects, groups, defaultProjectId: 'p-1' }
    })
    expect(result).toEqual(ipc)
  })

  it('syncProjects defaults groups to an empty list for existing callers', async () => {
    mockInvoke.mockResolvedValueOnce({ success: true, data: undefined })

    await syncProjects([], null)

    expect(mockInvoke).toHaveBeenCalledWith('remote_sync_projects', {
      payload: { projects: [], groups: [], defaultProjectId: null }
    })
  })
})
