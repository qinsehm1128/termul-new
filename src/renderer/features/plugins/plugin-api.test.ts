import type { ActionEvent } from '@shared/types/plugin.types'
import { invoke } from '@tauri-apps/api/core'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createTauriPluginApi } from './plugin-api'

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
  it('browser fallback never invokes commands, HTTP, or event transport', async () => {
    const transport = vi.fn(),
      event = vi.fn(),
      http = vi.spyOn(globalThis, 'fetch')
    const api = createTauriPluginApi({
      isDesktop: () => false,
      invokeDecoded: transport,
      onEvent: event
    })
    const results = await Promise.all([
      api.list(),
      api.install({ path: '/tmp/plugin' }),
      api.setEnabled({ id: action.pluginId, enabled: true }),
      api.reload({ id: action.pluginId }),
      api.uninstall({ id: action.pluginId }),
      api.getView({ id: action.pluginId, viewId: action.viewId }),
      api.dispatchAction(action),
      api.executeCommand({
        id: action.pluginId,
        generation: bootId,
        commandId: 'reset',
        args: null
      }),
      api.getConfig({ id: action.pluginId }),
      api.setConfig({ id: action.pluginId, expectedRevision: 1, values: {} })
    ])
    results.forEach((x) => {
      expect(x).toMatchObject({ success: false, code: 'PLUGIN_NOT_SUPPORTED' })
    })
    api.onChanged(vi.fn())()
    api.onConnectionChanged(vi.fn())()
    expect(transport).not.toHaveBeenCalled()
    expect(invoke).not.toHaveBeenCalled()
    expect(event).not.toHaveBeenCalled()
    expect(http).not.toHaveBeenCalled()
    http.mockRestore()
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
