import type { ActionEvent } from '@shared/types/plugin.types'
import { invoke } from '@tauri-apps/api/core'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { logFrontendError } from '@/lib/log-api'
import { createTauriPluginApi } from './plugin-api'
import { createWebPluginApi } from './web/web-plugin-api'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: () => false }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(async () => {}) }))
vi.mock('@/lib/host-events', () => ({ onHostEvent: vi.fn(() => () => {}) }))
const bootId = '77bac08a-bcf6-43c6-b0c2-cc971d2a980e'
const action: ActionEvent = {
  schemaVersion: 1,
  eventId: bootId,
  pluginId: 'org.termul.example',
  generation: bootId,
  viewId: 'counter',
  viewRevision: 1,
  nodeId: 'increment',
  actionId: 'increment',
  kind: 'click',
  value: null
}
beforeEach(() => vi.clearAllMocks())
describe('native plugin API', () => {
  it('treats unsupported host capability as a quiet optional feature', async () => {
    const api = createTauriPluginApi({
      isDesktop: () => true,
      invokeDecoded: vi.fn().mockResolvedValue({ success: false, code: 'PLUGIN_NOT_SUPPORTED' })
    })
    await api.getHostUi()
    await api.heartbeatHostContext({ sourceId: bootId })
    expect(logFrontendError).not.toHaveBeenCalled()
  })

  it('browser transport maps every plugin command to one same-origin HTTP request', async () => {
    const calls: { url: string; init: RequestInit }[] = []
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      calls.push({ url: String(input), init: init ?? {} })
      return new Response(
        JSON.stringify({ success: false, code: 'PLUGIN_NOT_FOUND', error: 'missing' }),
        { status: 200 }
      )
    })
    const api = createWebPluginApi({
      base: () => 'https://phone.example',
      fetch: fetchMock as unknown as typeof fetch
    })
    const processOwner = { id: action.pluginId, generation: bootId }
    const requests = [
      ['/plugins', 'GET', null, () => api.list()],
      ['/plugins/host-ui', 'GET', null, () => api.getHostUi()],
      [
        '/plugins/install',
        'POST',
        { path: '/tmp/plugin' },
        () => api.install({ path: '/tmp/plugin' })
      ],
      [
        '/plugins/enabled',
        'POST',
        { id: action.pluginId, enabled: true },
        () => api.setEnabled({ id: action.pluginId, enabled: true })
      ],
      [
        '/plugins/reload',
        'POST',
        { id: action.pluginId },
        () => api.reload({ id: action.pluginId })
      ],
      [
        '/plugins/uninstall',
        'POST',
        { id: action.pluginId },
        () => api.uninstall({ id: action.pluginId })
      ],
      [
        '/plugins/view',
        'POST',
        { id: action.pluginId, viewId: action.viewId },
        () => api.getView({ id: action.pluginId, viewId: action.viewId })
      ],
      ['/plugins/action', 'POST', action, () => api.dispatchAction(action)],
      [
        '/plugins/command',
        'POST',
        { id: action.pluginId, generation: bootId, commandId: 'reset', args: null },
        () =>
          api.executeCommand({
            id: action.pluginId,
            generation: bootId,
            commandId: 'reset',
            args: null
          })
      ],
      [
        '/plugins/config/get',
        'POST',
        { id: action.pluginId },
        () => api.getConfig({ id: action.pluginId })
      ],
      [
        '/plugins/config/set',
        'POST',
        { id: action.pluginId, expectedRevision: 1, values: {} },
        () => api.setConfig({ id: action.pluginId, expectedRevision: 1, values: {} })
      ],
      [
        '/plugins/host-ui/dismiss',
        'POST',
        { pluginId: action.pluginId, generation: bootId, kind: 'notification', id: 'note' },
        () =>
          api.dismissHostUi({
            pluginId: action.pluginId,
            generation: bootId,
            kind: 'notification',
            id: 'note'
          })
      ],
      [
        '/plugins/contribution',
        'POST',
        {
          pluginId: action.pluginId,
          generation: bootId,
          kind: 'status',
          contributionId: 'status',
          contextRevision: 1,
          target: {}
        },
        () =>
          api.executeContribution({
            pluginId: action.pluginId,
            generation: bootId,
            kind: 'status',
            contributionId: 'status',
            contextRevision: 1,
            target: {}
          })
      ],
      [
        '/plugins/web/open',
        'POST',
        { ...processOwner, viewId: 'web' },
        () => api.openWebSession({ ...processOwner, viewId: 'web' })
      ],
      [
        '/plugins/web/request',
        'POST',
        { sessionId: bootId, requestId: 'request', method: 'config.get', params: {} },
        () =>
          api.requestWeb({
            sessionId: bootId,
            requestId: 'request',
            method: 'config.get',
            params: {}
          })
      ],
      [
        '/plugins/web/read',
        'POST',
        { sessionId: bootId, afterSequence: 0 },
        () => api.readWebSession({ sessionId: bootId, afterSequence: 0 })
      ],
      [
        '/plugins/web/close',
        'POST',
        { sessionId: bootId },
        () => api.closeWebSession({ sessionId: bootId })
      ],
      ['/plugins/processes', 'POST', processOwner, () => api.listProcesses(processOwner)],
      [
        '/plugins/processes/logs',
        'POST',
        { ...processOwner, processId: 'server', tail: 200 },
        () => api.getProcessLogs({ ...processOwner, processId: 'server', tail: 200 })
      ],
      [
        '/plugins/processes/control',
        'POST',
        { ...processOwner, processId: 'server', action: 'restart' },
        () => api.controlProcess({ ...processOwner, processId: 'server', action: 'restart' })
      ]
    ] as const
    for (const [path, method, payload, invoke] of requests) {
      expect(await invoke()).toMatchObject({ success: false, code: 'PLUGIN_NOT_FOUND' })
      const last = calls[calls.length - 1]
      expect(last.url).toBe(`https://phone.example${path}`)
      expect(last.init.method).toBe(method)
      expect(last.init.headers).toBeInstanceOf(Headers)
      if (method === 'POST') expect(JSON.parse(last.init.body as string)).toEqual(payload)
    }
    // Host context is desktop-only: unsupported, and never sent over HTTP.
    expect(await api.publishHostContext({ sourceId: bootId } as never)).toMatchObject({
      success: false,
      code: 'PLUGIN_NOT_SUPPORTED'
    })
    expect(await api.heartbeatHostContext({ sourceId: bootId } as never)).toMatchObject({
      success: false,
      code: 'PLUGIN_NOT_SUPPORTED'
    })
    expect(fetchMock).toHaveBeenCalledTimes(requests.length)
    expect(invoke).not.toHaveBeenCalled()

    // Shared WebSocket events are consumed through the ACP transport, never a new socket.
    const listeners = new Map<string, (payload: unknown) => void>()
    const events = createWebPluginApi({
      fetch: fetchMock as unknown as typeof fetch,
      onEvent: (name, handler) => {
        listeners.set(name, handler)
        return Object.assign(() => undefined, { ready: Promise.resolve() })
      }
    })
    const changed = vi.fn()
    const stop = events.onChanged(changed)
    await stop.ready
    listeners.get('plugins:changed')!({
      schemaVersion: 1,
      bootId,
      seq: 1,
      type: 'registry_changed'
    })
    expect(changed).toHaveBeenCalledOnce()
    stop()
  })
  it('uses the decoded IpcResult path with exact commands and payload wrapping', async () => {
    vi.mocked(invoke).mockResolvedValue({
      success: true,
      data: { schemaVersion: 1, bootId, seq: 0, plugins: [] }
    })
    const api = createTauriPluginApi({ isDesktop: () => true })
    expect(await api.list()).toEqual({
      success: true,
      data: { schemaVersion: 1, bootId, seq: 0, plugins: [] }
    })
    expect(invoke).toHaveBeenLastCalledWith('plugin_list', undefined)
    vi.mocked(invoke).mockResolvedValue({
      success: false,
      code: 'PLUGIN_NOT_FOUND',
      error: 'missing'
    })
    expect(await api.getHostUi()).toMatchObject({ success: false, code: 'PLUGIN_NOT_FOUND' })
    // The Tauri command requires a payload argument; omitting it fails every call.
    expect(invoke).toHaveBeenLastCalledWith('plugin_get_host_ui', { payload: {} })
    const requests = [
      ['plugin_install', { path: '/tmp/native' }, () => api.install({ path: '/tmp/native' })],
      [
        'plugin_set_enabled',
        { id: action.pluginId, enabled: true },
        () => api.setEnabled({ id: action.pluginId, enabled: true })
      ],
      ['plugin_reload', { id: action.pluginId }, () => api.reload({ id: action.pluginId })],
      ['plugin_uninstall', { id: action.pluginId }, () => api.uninstall({ id: action.pluginId })],
      [
        'plugin_get_view',
        { id: action.pluginId, viewId: action.viewId },
        () => api.getView({ id: action.pluginId, viewId: action.viewId })
      ],
      ['plugin_dispatch_action', action, () => api.dispatchAction(action)],
      [
        'plugin_execute_command',
        { id: action.pluginId, generation: bootId, commandId: 'reset', args: null },
        () =>
          api.executeCommand({
            id: action.pluginId,
            generation: bootId,
            commandId: 'reset',
            args: null
          })
      ],
      ['plugin_get_config', { id: action.pluginId }, () => api.getConfig({ id: action.pluginId })],
      [
        'plugin_set_config',
        { id: action.pluginId, expectedRevision: 1, values: {} },
        () => api.setConfig({ id: action.pluginId, expectedRevision: 1, values: {} })
      ],
      [
        'plugin_publish_host_context',
        {
          sourceId: bootId,
          sourceSequence: 1,
          reset: true,
          snapshot: {
            activeProject: null,
            projectIds: [],
            activeTerminalId: null,
            activeSessionId: null,
            terminals: [],
            agentSessions: [],
            focus: { textInput: false, terminal: false },
            truncated: false
          },
          events: []
        },
        () =>
          api.publishHostContext({
            sourceId: bootId,
            sourceSequence: 1,
            reset: true,
            snapshot: {
              activeProject: null,
              projectIds: [],
              activeTerminalId: null,
              activeSessionId: null,
              terminals: [],
              agentSessions: [],
              focus: { textInput: false, terminal: false },
              truncated: false
            },
            events: []
          })
      ],
      [
        'plugin_heartbeat_host_context',
        { sourceId: bootId },
        () => api.heartbeatHostContext({ sourceId: bootId })
      ],
      [
        'plugin_dismiss_host_ui',
        { pluginId: action.pluginId, generation: bootId, kind: 'notification', id: 'note' },
        () =>
          api.dismissHostUi({
            pluginId: action.pluginId,
            generation: bootId,
            kind: 'notification',
            id: 'note'
          })
      ],
      [
        'plugin_execute_contribution',
        {
          pluginId: action.pluginId,
          generation: bootId,
          kind: 'status',
          contributionId: 'status',
          contextRevision: 1,
          target: {}
        },
        () =>
          api.executeContribution({
            pluginId: action.pluginId,
            generation: bootId,
            kind: 'status',
            contributionId: 'status',
            contextRevision: 1,
            target: {}
          })
      ]
    ] as const
    for (const [command, payload, call] of requests) {
      expect(await call()).toMatchObject({ success: false, code: 'PLUGIN_NOT_FOUND' })
      expect(invoke).toHaveBeenLastCalledWith(command, { payload })
    }
  })
  it('normalizes malformed data/envelopes instead of leaking exceptions', async () => {
    const api = createTauriPluginApi({ isDesktop: () => true })
    for (const response of [
      { success: true, data: { schemaVersion: 2 } },
      { success: true },
      { data: [] },
      null
    ]) {
      vi.mocked(invoke).mockResolvedValue(response)
      expect(await api.list()).toMatchObject({ success: false, code: 'NETWORK_ERROR' })
    }
    vi.mocked(invoke).mockRejectedValue({ code: 'PLUGIN_CORE_DISCONNECTED', message: 'gone' })
    expect(await api.list()).toEqual({
      success: false,
      code: 'PLUGIN_CORE_DISCONNECTED',
      error: 'gone'
    })
    expect(invoke).toHaveBeenCalledTimes(5)
  })
  it('strictly decodes events and preserves unsubscription', () => {
    const listeners = new Map<string, (payload: unknown) => void>(),
      off = vi.fn()
    const api = createTauriPluginApi({
      isDesktop: () => true,
      onEvent: (name, callback) => {
        listeners.set(name, callback)
        return off
      }
    })
    const changed = vi.fn(),
      connection = vi.fn()
    const stop = api.onChanged(changed)
    api.onConnectionChanged(connection)
    listeners.get('plugins:changed')!({
      schemaVersion: 1,
      bootId,
      seq: 1,
      type: 'registry_changed'
    })
    listeners.get('plugins:changed')!({ schemaVersion: 1, bootId, seq: 2, type: 'unknown' })
    listeners.get('plugins:connection_changed')!({ state: 'disconnected', bootId: null })
    expect(changed).toHaveBeenCalledTimes(1)
    expect(connection).toHaveBeenCalledWith({ state: 'disconnected', bootId: null })
    stop()
    expect(off).toHaveBeenCalledOnce()
  })
})
