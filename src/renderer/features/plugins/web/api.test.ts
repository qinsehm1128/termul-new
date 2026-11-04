import { invoke } from '@tauri-apps/api/core'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createTauriPluginApi } from '../plugin-api'
import { webSession } from './test-fixtures'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: () => false }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(async () => {}) }))
beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(invoke).mockReset()
})
const owner = { id: webSession.pluginId, generation: webSession.generation }

describe('web and managed process facade', () => {
  it('performs zero invoke/listen for every new browser-mode API', async () => {
    const listen = vi.fn()
    const call = vi.fn()
    const api = createTauriPluginApi({
      isDesktop: () => false,
      invokeDecoded: call,
      onEvent: listen
    })
    const results = await Promise.all([
      api.openWebSession({ ...owner, viewId: 'web' }),
      api.closeWebSession({ sessionId: webSession.sessionId }),
      api.readWebSession({ sessionId: webSession.sessionId, afterSequence: 0 }),
      api.requestWeb({
        sessionId: webSession.sessionId,
        requestId: 'request',
        method: 'config.get',
        params: {}
      }),
      api.listProcesses(owner),
      api.getProcessLogs({ ...owner, processId: 'server', tail: 200 }),
      api.controlProcess({ ...owner, processId: 'server', action: 'restart' })
    ])
    api.onChanged(vi.fn())()
    api.onConnectionChanged(vi.fn())()
    expect(
      results.every((result) => !result.success && result.code === 'PLUGIN_NOT_SUPPORTED')
    ).toBe(true)
    expect(call).not.toHaveBeenCalled()
    expect(listen).not.toHaveBeenCalled()
  })

  it('binds session URLs to approved origins and exact generation/session entry paths', async () => {
    const call = vi.mocked(invoke).mockResolvedValue({ success: true, data: webSession })
    const api = createTauriPluginApi({ isDesktop: () => true })
    expect(await api.openWebSession({ ...owner, viewId: 'web' })).toEqual({
      success: true,
      data: webSession
    })
    expect(call).toHaveBeenCalledWith('plugin_web_open', { payload: { ...owner, viewId: 'web' } })
    for (const url of [
      'https://attacker.invalid/ui/index.html',
      'http://127.0.0.1:1234/ui/index.html',
      webSession.url.replace('/ui/index.html', '/ui/other.html'),
      `${webSession.url}?capability=secret`
    ]) {
      call.mockResolvedValueOnce({ success: true, data: { ...webSession, url } })
      expect(await api.openWebSession({ ...owner, viewId: 'web' })).toMatchObject({
        success: false,
        code: 'NETWORK_ERROR'
      })
    }
    call.mockResolvedValueOnce({
      success: true,
      data: {
        ...webSession,
        url: webSession.url.replace('se-plugin://localhost', 'http://se-plugin.localhost')
      }
    })
    expect(await api.openWebSession({ ...owner, viewId: 'web' })).toMatchObject({ success: true })
  })

  it('uses strict W1 decoders and stable command/payload names for web methods', async () => {
    const call = vi.mocked(invoke)
    const api = createTauriPluginApi({ isDesktop: () => true })
    call.mockResolvedValueOnce({
      success: true,
      data: {
        sequence: 1,
        resyncRequired: false,
        events: [{ sequence: 1, type: 'message', payload: { kind: 'done' } }]
      }
    })
    await expect(
      api.readWebSession({ sessionId: webSession.sessionId, afterSequence: 0 })
    ).resolves.toMatchObject({ success: true })
    expect(call).toHaveBeenLastCalledWith('plugin_web_read', {
      payload: { sessionId: webSession.sessionId, afterSequence: 0 }
    })
    call.mockResolvedValueOnce({ success: true, data: { closed: false } })
    await expect(api.closeWebSession({ sessionId: webSession.sessionId })).resolves.toMatchObject({
      success: false,
      code: 'NETWORK_ERROR'
    })
    call.mockResolvedValueOnce({ success: true, data: { committed: true } })
    const request = {
      sessionId: webSession.sessionId,
      requestId: 'request',
      method: 'config.get' as const,
      params: {}
    }
    await expect(api.requestWeb(request)).resolves.toEqual({
      success: true,
      data: { committed: true }
    })
    expect(call).toHaveBeenLastCalledWith('plugin_web_request', { payload: request })
  })

  it('decodes process status/logs and rejects malformed, duplicate or extra fields', async () => {
    const status = { state: 'running', pid: 123, restarts: 1, startedAt: 1 }
    const call = vi
      .mocked(invoke)
      .mockResolvedValueOnce({ success: true, data: { processes: [{ id: 'server', status }] } })
    const api = createTauriPluginApi({ isDesktop: () => true })
    expect(await api.listProcesses(owner)).toEqual({
      success: true,
      data: { processes: [{ id: 'server', status }] }
    })
    expect(call).toHaveBeenLastCalledWith('plugin_process_list', { payload: owner })
    call.mockResolvedValueOnce({ success: true, data: status })
    const control = { ...owner, processId: 'server', action: 'restart' as const }
    expect(await api.controlProcess(control)).toEqual({ success: true, data: status })
    expect(call).toHaveBeenLastCalledWith('plugin_process_control', { payload: control })
    const logs = { lines: [{ stream: 'stdout', at: 1, text: 'ready' }], truncated: false }
    call.mockResolvedValueOnce({ success: true, data: logs })
    expect(await api.getProcessLogs({ ...owner, processId: 'server', tail: 200 })).toEqual({
      success: true,
      data: logs
    })
    expect(call).toHaveBeenLastCalledWith('plugin_process_logs', {
      payload: { ...owner, processId: 'server', tail: 200 }
    })
    for (const data of [
      {
        processes: [
          { id: 'server', status },
          { id: 'server', status }
        ]
      },
      { processes: [], extra: true },
      { processes: [{ id: 'server', status: { ...status, state: 'invented' } }] }
    ]) {
      call.mockResolvedValueOnce({ success: true, data })
      expect(await api.listProcesses(owner)).toMatchObject({
        success: false,
        code: 'NETWORK_ERROR'
      })
    }
  })
})
