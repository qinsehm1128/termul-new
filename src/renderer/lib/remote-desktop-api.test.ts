import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  REMOTE_DESKTOP_STATE_EVENT,
  type RemoteDesktopState,
  remoteDesktopApi,
  webRemoteDesktopApi
} from './remote-desktop-api'

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  log: vi.fn(),
  desktop: true
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('./tauri-event', () => ({ listen: mocks.listen }))
vi.mock('./tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('./log-api', () => ({ logFrontendError: mocks.log }))
const settings = {
  supported: true,
  viewEnabled: true,
  controlEnabled: false,
  permissions: { screenRecording: true, accessibility: false }
}

beforeEach(() => {
  vi.resetAllMocks()
  mocks.desktop = true
  mocks.log.mockResolvedValue(undefined)
})
afterEach(() => vi.unstubAllGlobals())

describe('remote desktop host facade', () => {
  it('invokes exact native commands with independent camelCase switches', async () => {
    mocks.invoke.mockResolvedValue(settings)
    expect(await remoteDesktopApi.getSettings()).toEqual(settings)
    expect(
      await remoteDesktopApi.setSettings({ viewEnabled: false, controlEnabled: true })
    ).toEqual(settings)
    await remoteDesktopApi.emergencyStop()
    expect(mocks.invoke.mock.calls).toEqual([
      ['get_remote_desktop_settings'],
      ['set_remote_desktop_settings', { viewEnabled: false, controlEnabled: true }],
      ['remote_desktop_emergency_stop']
    ])
  })

  it('implements explicit local web GET/PUT and stop POST parity', async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(settings)))
      .mockResolvedValueOnce(new Response(JSON.stringify(settings)))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true })))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
    vi.stubGlobal('fetch', fetcher)
    expect(await webRemoteDesktopApi.getSettings()).toEqual(settings)
    expect(
      await webRemoteDesktopApi.setSettings({ viewEnabled: true, controlEnabled: false })
    ).toEqual(settings)
    await webRemoteDesktopApi.emergencyStop()
    await webRemoteDesktopApi.emergencyStop()
    expect(fetcher.mock.calls.map(([url, init]) => [new URL(url).pathname, init.method])).toEqual([
      ['/api/remote-desktop/settings', 'GET'],
      ['/api/remote-desktop/settings', 'PUT'],
      ['/api/remote-desktop/stop', 'POST'],
      ['/api/remote-desktop/stop', 'POST']
    ])
    expect(JSON.parse(fetcher.mock.calls[1][1].body)).toEqual({
      viewEnabled: true,
      controlEnabled: false
    })
    expect(mocks.invoke).not.toHaveBeenCalled()
  })

  it('explicitly gates browser reads, mutations and subscriptions without host/network side effects', async () => {
    mocks.desktop = false
    const fetcher = vi.fn()
    vi.stubGlobal('fetch', fetcher)
    expect(await remoteDesktopApi.getSettings()).toEqual({
      supported: false,
      viewEnabled: false,
      controlEnabled: false,
      permissions: { screenRecording: false, accessibility: false }
    })
    await expect(
      remoteDesktopApi.setSettings({ viewEnabled: true, controlEnabled: true })
    ).rejects.toThrow('桌面客户端')
    await expect(remoteDesktopApi.emergencyStop()).rejects.toThrow('桌面客户端')
    const cleanup = await remoteDesktopApi.subscribeState(vi.fn())
    cleanup()
    expect(mocks.invoke).not.toHaveBeenCalled()
    expect(mocks.listen).not.toHaveBeenCalled()
    expect(fetcher).not.toHaveBeenCalled()
  })

  it('forwards native state payloads and unsubscription exactly', async () => {
    const cleanup = vi.fn()
    const callback = vi.fn()
    mocks.listen.mockResolvedValue(cleanup)
    expect(await remoteDesktopApi.subscribeState(callback)).toBe(cleanup)
    expect(mocks.listen.mock.calls[0][0]).toBe(REMOTE_DESKTOP_STATE_EVENT)
    const payload: RemoteDesktopState = {
      viewers: 1,
      controllers: 0,
      display: '2',
      controlEnabled: false
    }
    mocks.listen.mock.calls[0][1]({ payload })
    expect(callback).toHaveBeenCalledExactlyOnceWith(payload)
  })

  it('sanitizes native failures and logs no backend secrets', async () => {
    mocks.invoke.mockRejectedValue(new Error('private-token secret host path'))
    await expect(remoteDesktopApi.emergencyStop()).rejects.toThrow('远程桌面操作失败')
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('private-token')
    expect(mocks.log).toHaveBeenCalledWith({
      source: 'remote-desktop-api',
      message: 'remote_desktop_emergency_stop: failed'
    })
  })

  it('rejects failed web responses without exposing their bodies', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(new Response('sensitive error', { status: 403 }))
    )
    await expect(
      webRemoteDesktopApi.setSettings({ viewEnabled: false, controlEnabled: false })
    ).rejects.toThrow('远程桌面操作失败')
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('sensitive error')
  })
})
