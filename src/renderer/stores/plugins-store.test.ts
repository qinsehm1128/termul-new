import type { PluginEvent, PluginInfo } from '@shared/types/plugin.types'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { pluginApi } from '@/features/plugins/plugin-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { usePluginsStore } from './plugins-store'

vi.mock('@/features/plugins/plugin-api', () => ({
  pluginApi: {
    list: vi.fn(),
    install: vi.fn(),
    setEnabled: vi.fn(),
    reload: vi.fn(),
    uninstall: vi.fn(),
    onChanged: vi.fn(),
    onConnectionChanged: vi.fn()
  }
}))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: vi.fn(() => true) }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(async () => {}) }))

const plugin: PluginInfo = {
  id: 'org.termul.test',
  desiredEnabled: true,
  state: 'active',
  generation: 'generation-1',
  configRevision: 1,
  sha256: 'a'.repeat(64),
  lastError: null,
  manifest: {
    schemaVersion: 1,
    id: 'org.termul.test',
    name: 'Test plugin',
    version: '1.0.0',
    abi: { min: 1, max: 1 },
    services: { provides: [], requires: [] },
    contributes: { commands: [], configuration: { title: '', properties: {} }, views: [] }
  }
}
const snapshot = (seq = 0, bootId = 'boot-1', plugins = [plugin]) => ({
  success: true as const,
  data: { schemaVersion: 1 as const, seq, bootId, plugins }
})
const flush = async () => {
  await Promise.resolve()
  await Promise.resolve()
}
let dispose: (() => void) | undefined
let changed: (event: PluginEvent) => void
let connection: (event: { state: 'connected' | 'disconnected'; bootId: string | null }) => void
const offChanged = vi.fn(),
  offConnection = vi.fn()

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(isTauriContext).mockReturnValue(true)
  usePluginsStore.setState({
    plugins: [],
    connected: false,
    loading: false,
    busy: null,
    error: null,
    bootId: null,
    seq: 0
  })
  vi.mocked(pluginApi.list).mockResolvedValue(snapshot())
  vi.mocked(pluginApi.onChanged).mockImplementation((callback) => {
    changed = callback
    return offChanged
  })
  vi.mocked(pluginApi.onConnectionChanged).mockImplementation((callback) => {
    connection = callback
    return offConnection
  })
})
afterEach(() => {
  dispose?.()
  dispose = undefined
})

describe('plugins store', () => {
  it('subscribes before snapshot, shares one subscription, and disposes only after the final consumer', async () => {
    dispose = usePluginsStore.getState().subscribe()
    const second = usePluginsStore.getState().subscribe()
    await flush()
    expect(pluginApi.onChanged).toHaveBeenCalledOnce()
    expect(pluginApi.onConnectionChanged).toHaveBeenCalledOnce()
    expect(vi.mocked(pluginApi.onChanged).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(pluginApi.list).mock.invocationCallOrder[0]!
    )
    expect(vi.mocked(pluginApi.onConnectionChanged).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(pluginApi.list).mock.invocationCallOrder[0]!
    )
    expect(usePluginsStore.getState()).toMatchObject({ plugins: [plugin], connected: true })
    second()
    expect(offChanged).not.toHaveBeenCalled()
    dispose()
    dispose()
    expect(offChanged).toHaveBeenCalledOnce()
    expect(offConnection).toHaveBeenCalledOnce()
  })

  it('recovers sequence gaps, boot changes, resync and connection changes through full snapshots', async () => {
    dispose = usePluginsStore.getState().subscribe()
    await flush()
    vi.mocked(pluginApi.list).mockResolvedValue(snapshot(4))
    changed({ schemaVersion: 1, bootId: 'boot-1', seq: 4, type: 'view_changed' })
    await flush()
    expect(pluginApi.list).toHaveBeenCalledTimes(2)
    changed({ schemaVersion: 1, bootId: 'boot-1', seq: 4, type: 'state_changed' })
    await flush()
    expect(pluginApi.list).toHaveBeenCalledTimes(2)
    vi.mocked(pluginApi.list).mockResolvedValue(snapshot(1, 'boot-2'))
    changed({ schemaVersion: 1, bootId: 'boot-2', seq: 1, type: 'resync_required' })
    await flush()
    expect(usePluginsStore.getState().bootId).toBe('boot-2')
    connection({ state: 'disconnected', bootId: null })
    await flush()
    expect(usePluginsStore.getState().connected).toBe(false)
    connection({ state: 'connected', bootId: 'boot-2' })
    await flush()
    expect(usePluginsStore.getState().connected).toBe(true)
    expect(pluginApi.list).toHaveBeenCalledTimes(5)
  })

  it('does not let a stale in-flight list overwrite a newer event snapshot', async () => {
    let resolve!: (value: ReturnType<typeof snapshot>) => void
    vi.mocked(pluginApi.list).mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      })
    )
    dispose = usePluginsStore.getState().subscribe()
    vi.mocked(pluginApi.list).mockResolvedValue(snapshot(2, 'boot-2', []))
    changed({ schemaVersion: 1, bootId: 'boot-2', seq: 2, type: 'registry_changed' })
    await flush()
    resolve(snapshot())
    await flush()
    expect(usePluginsStore.getState()).toMatchObject({
      bootId: 'boot-2',
      seq: 2,
      plugins: [],
      connected: true
    })
  })

  it('removes stale executable commands immediately during lifecycle reconciliation', async () => {
    dispose = usePluginsStore.getState().subscribe()
    await flush()
    vi.mocked(pluginApi.list).mockReturnValue(new Promise(() => {}))
    changed({
      schemaVersion: 1,
      bootId: 'boot-1',
      seq: 1,
      type: 'state_changed',
      pluginId: plugin.id
    })
    expect(usePluginsStore.getState().connected).toBe(false)
  })

  it('runs install, enable, reload with/without path, disable and uninstall; reports busy and structured errors', async () => {
    dispose = usePluginsStore.getState().subscribe()
    await flush()
    vi.mocked(pluginApi.install).mockResolvedValue({ success: true, data: plugin })
    vi.mocked(pluginApi.setEnabled).mockResolvedValue({ success: true, data: plugin })
    vi.mocked(pluginApi.reload).mockResolvedValue({ success: true, data: plugin })
    vi.mocked(pluginApi.uninstall).mockResolvedValue({
      success: true,
      data: { id: plugin.id, removed: true }
    })
    await usePluginsStore.getState().install('/plugin.dylib')
    await usePluginsStore.getState().setEnabled(plugin.id, true)
    await usePluginsStore.getState().reload(plugin.id)
    expect(pluginApi.reload).toHaveBeenLastCalledWith({ id: plugin.id })
    await usePluginsStore.getState().reload(plugin.id, '/upgrade.so')
    expect(pluginApi.reload).toHaveBeenLastCalledWith({ id: plugin.id, path: '/upgrade.so' })
    await usePluginsStore.getState().setEnabled(plugin.id, false)
    vi.mocked(pluginApi.list).mockResolvedValue(snapshot(2, 'boot-1', []))
    await usePluginsStore.getState().uninstall(plugin.id)
    expect(usePluginsStore.getState().plugins).toEqual([])
    let finish!: (value: { success: false; code: string; error: string }) => void
    vi.mocked(pluginApi.install).mockReturnValue(
      new Promise((done) => {
        finish = done
      })
    )
    const pending = usePluginsStore.getState().install('/bad.dll')
    expect(usePluginsStore.getState().busy).toBe('install')
    finish({ success: false, code: 'PLUGIN_ABI_UNSUPPORTED', error: 'unsafe details' })
    await pending
    expect(usePluginsStore.getState()).toMatchObject({
      busy: null,
      error: { code: 'PLUGIN_ABI_UNSUPPORTED', message: 'Plugin operation failed' }
    })
  })

  it('shows list failure without leaking plugin JSON and allows retry', async () => {
    vi.mocked(pluginApi.list).mockResolvedValueOnce({
      success: false,
      code: 'PLUGIN_CORE_UNAVAILABLE',
      error: 'secret JSON'
    })
    dispose = usePluginsStore.getState().subscribe()
    await flush()
    expect(usePluginsStore.getState()).toMatchObject({
      loading: false,
      connected: false,
      error: { code: 'PLUGIN_CORE_UNAVAILABLE', message: 'Plugin list failed' }
    })
    await usePluginsStore.getState().load()
    expect(usePluginsStore.getState().connected).toBe(true)
  })

  it('retries a snapshot behind the event cursor and preserves the newest sequence', async () => {
    dispose = usePluginsStore.getState().subscribe()
    await flush()
    vi.mocked(pluginApi.list).mockResolvedValueOnce(snapshot(1)).mockResolvedValueOnce(snapshot(3))
    changed({ schemaVersion: 1, bootId: 'boot-1', seq: 3, type: 'resync_required' })
    await flush()
    await flush()
    expect(pluginApi.list).toHaveBeenCalledTimes(3)
    expect(usePluginsStore.getState()).toMatchObject({ seq: 3, connected: true, loading: false })
  })

  it('keeps connected settings mounted while reconciling a contiguous config change', async () => {
    dispose = usePluginsStore.getState().subscribe()
    await flush()
    vi.mocked(pluginApi.list).mockReturnValue(new Promise(() => {}))
    changed({
      schemaVersion: 1,
      bootId: 'boot-1',
      seq: 1,
      type: 'config_changed',
      pluginId: plugin.id
    })
    expect(usePluginsStore.getState()).toMatchObject({ connected: true, loading: true })
    expect(pluginApi.list).toHaveBeenCalledTimes(2)
  })

  it('browser performs zero API calls, subscriptions, invoke and HTTP', async () => {
    vi.mocked(isTauriContext).mockReturnValue(false)
    const fetch = vi.spyOn(globalThis, 'fetch')
    dispose = usePluginsStore.getState().subscribe()
    const store = usePluginsStore.getState()
    await store.load()
    await store.install('/p.dll')
    await store.setEnabled(plugin.id, true)
    await store.reload(plugin.id)
    await store.uninstall(plugin.id)
    for (const method of Object.values(pluginApi)) expect(method).not.toHaveBeenCalled()
    expect(fetch).not.toHaveBeenCalled()
    fetch.mockRestore()
  })
})
