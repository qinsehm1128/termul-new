import { listen } from '@tauri-apps/api/event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { pluginApi } from '@/features/plugins/plugin-api'
import { usePluginsStore } from './plugins-store'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(async () => {}) }))

interface Registration {
  resolve: (stop: () => void) => void
  reject: (error: Error) => void
}
const registrations = new Map<string, Registration>()
const flush = async () => {
  for (let step = 0; step < 10; step += 1) await Promise.resolve()
}
let dispose: (() => void) | undefined

beforeEach(() => {
  registrations.clear()
  vi.mocked(listen).mockImplementation(
    (name) =>
      new Promise((resolve, reject) => {
        registrations.set(name, { resolve, reject })
      })
  )
  vi.spyOn(pluginApi, 'list').mockResolvedValue({
    success: true,
    data: { schemaVersion: 1, bootId: 'boot-1', seq: 0, plugins: [] }
  })
  usePluginsStore.setState({ connected: false, loading: false, error: null })
})
afterEach(() => {
  dispose?.()
  dispose = undefined
  vi.restoreAllMocks()
  vi.clearAllMocks()
})

describe('plugin store native listener readiness', () => {
  it('requests no snapshot until both Tauri registrations resolve', async () => {
    dispose = usePluginsStore.getState().subscribe()
    await flush()
    expect(listen).toHaveBeenCalledTimes(2)
    expect(pluginApi.list).not.toHaveBeenCalled()
    registrations.get('plugins:changed')!.resolve(vi.fn())
    await flush()
    expect(pluginApi.list).not.toHaveBeenCalled()
    registrations.get('plugins:connection_changed')!.resolve(vi.fn())
    await flush()
    expect(pluginApi.list).toHaveBeenCalledOnce()
    expect(usePluginsStore.getState().connected).toBe(true)
  })

  it('unregisters both late listeners without loading after teardown', async () => {
    const offChanged = vi.fn(),
      offConnection = vi.fn()
    dispose = usePluginsStore.getState().subscribe()
    dispose()
    registrations.get('plugins:changed')!.resolve(offChanged)
    registrations.get('plugins:connection_changed')!.resolve(offConnection)
    await flush()
    expect(pluginApi.list).not.toHaveBeenCalled()
    expect(offChanged).toHaveBeenCalledOnce()
    expect(offConnection).toHaveBeenCalledOnce()
    dispose()
    expect(offChanged).toHaveBeenCalledOnce()
    expect(offConnection).toHaveBeenCalledOnce()
  })

  it('cleans up a partially registered subscription and permits a new subscriber', async () => {
    const oldChanged = vi.fn(),
      oldConnection = vi.fn()
    dispose = usePluginsStore.getState().subscribe()
    const oldRegistrations = new Map(registrations)
    oldRegistrations.get('plugins:changed')!.resolve(oldChanged)
    await flush()
    dispose()
    expect(oldChanged).toHaveBeenCalledOnce()
    dispose = usePluginsStore.getState().subscribe()
    oldRegistrations.get('plugins:connection_changed')!.resolve(oldConnection)
    await flush()
    expect(oldConnection).toHaveBeenCalledOnce()
    expect(pluginApi.list).not.toHaveBeenCalled()
    registrations.get('plugins:changed')!.resolve(vi.fn())
    registrations.get('plugins:connection_changed')!.resolve(vi.fn())
    await flush()
    expect(pluginApi.list).toHaveBeenCalledOnce()
  })

  it('re-registers both listeners before Refresh can restore connected state after failure', async () => {
    dispose = usePluginsStore.getState().subscribe()
    const oldConnection = vi.fn()
    registrations.get('plugins:changed')!.reject(new Error('registration failed'))
    registrations.get('plugins:connection_changed')!.resolve(oldConnection)
    await flush()
    expect(oldConnection).toHaveBeenCalledOnce()
    expect(usePluginsStore.getState().connected).toBe(false)
    const refresh = usePluginsStore.getState().load()
    await flush()
    expect(listen).toHaveBeenCalledTimes(4)
    expect(pluginApi.list).not.toHaveBeenCalled()
    expect(usePluginsStore.getState().connected).toBe(false)
    registrations.get('plugins:changed')!.resolve(vi.fn())
    await flush()
    expect(pluginApi.list).not.toHaveBeenCalled()
    registrations.get('plugins:connection_changed')!.resolve(vi.fn())
    await refresh
    expect(pluginApi.list).toHaveBeenCalledOnce()
    expect(usePluginsStore.getState()).toMatchObject({ connected: true, error: null })
  })

  it('cleans up a late listener when the other registration fails', async () => {
    const offConnection = vi.fn()
    dispose = usePluginsStore.getState().subscribe()
    registrations.get('plugins:changed')!.reject(new Error('registration failed'))
    await flush()
    registrations.get('plugins:connection_changed')!.resolve(offConnection)
    await flush()
    expect(pluginApi.list).not.toHaveBeenCalled()
    expect(offConnection).toHaveBeenCalledOnce()
    expect(usePluginsStore.getState()).toMatchObject({
      connected: false,
      loading: false,
      error: { code: 'PLUGIN_IO', message: 'Plugin subscription failed' }
    })
  })
})
