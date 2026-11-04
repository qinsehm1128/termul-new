import type {
  HostUiSnapshot,
  PluginConnectionChanged,
  PluginEvent
} from '@shared/types/plugin.types'
import { describe, expect, it, vi } from 'vitest'
import type { HostEventSubscription } from '@/lib/host-events'
import { createPluginHostUiStore } from './host-ui-store'
import type { TauriPluginApi } from './plugin-api'

function fixture(options: { ready?: Promise<void>; desktop?: boolean } = {}) {
  let changed!: (event: PluginEvent) => void
  let connection!: (event: PluginConnectionChanged) => void
  const unsubscribe = vi.fn()
  const listen = <T>(capture: (handler: T) => void) =>
    vi.fn((handler: T) => {
      capture(handler)
      return Object.assign(unsubscribe, {
        ready: options.ready ?? Promise.resolve()
      }) as HostEventSubscription
    })
  const snapshot = (revision: number, bootId = 'boot'): HostUiSnapshot => ({
    schemaVersion: 1,
    bootId,
    revision,
    notifications: [],
    progress: [],
    statusItems: []
  })
  const api = {
    getHostUi: vi.fn().mockResolvedValue({ success: true, data: snapshot(1) }),
    dismissHostUi: vi.fn().mockResolvedValue({ success: true, data: null }),
    executeContribution: vi
      .fn()
      .mockResolvedValue({ success: true, data: { schemaVersion: 1, result: null } }),
    onChanged: listen<(event: PluginEvent) => void>((handler) => {
      changed = handler
    }),
    onConnectionChanged: listen<(event: PluginConnectionChanged) => void>((handler) => {
      connection = handler
    })
  }
  const store = createPluginHostUiStore(
    api as unknown as TauriPluginApi,
    () => options.desktop ?? true
  )
  return {
    store,
    api,
    unsubscribe,
    snapshot,
    changed: (event: Partial<PluginEvent>) =>
      changed({
        bootId: 'boot',
        seq: 1,
        type: 'host_ui_changed',
        pluginId: 'plugin',
        generation: 'gen',
        ...event
      } as PluginEvent),
    connection: (state: 'connected' | 'disconnected', bootId: string | null = 'boot') =>
      connection({ state, bootId } as PluginConnectionChanged)
  }
}
const settle = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve()
}

describe('host UI revision cache', () => {
  it('stops unsupported UI fetches across events and repeated connected messages until reconnect', async () => {
    const f = fixture()
    f.api.getHostUi.mockResolvedValue({ success: false, code: 'PLUGIN_NOT_SUPPORTED' })
    const dispose = f.store.getState().subscribe()
    await settle()
    for (let seq = 2; seq < 20; seq++) {
      f.changed({ seq })
      f.connection('connected')
      await f.store.getState().load()
    }
    await settle()
    expect(f.api.getHostUi).toHaveBeenCalledTimes(1)
    expect(f.store.getState().snapshot).toBeNull()
    f.connection('disconnected')
    f.connection('connected')
    await settle()
    expect(f.api.getHostUi).toHaveBeenCalledTimes(2)
    dispose()
  })

  it('fetches only after listener readiness and reloads host_ui_changed without regressing revisions', async () => {
    let ready!: () => void
    const f = fixture({
      ready: new Promise<void>((resolve) => {
        ready = resolve
      })
    })
    const dispose = f.store.getState().subscribe()
    expect(f.api.getHostUi).not.toHaveBeenCalled()
    ready()
    await settle()
    expect(f.store.getState().snapshot?.revision).toBe(1)
    f.api.getHostUi.mockResolvedValueOnce({ success: true, data: f.snapshot(3) })
    f.changed({ seq: 2 })
    await settle()
    expect(f.store.getState().snapshot?.revision).toBe(3)
    f.api.getHostUi.mockResolvedValueOnce({ success: true, data: f.snapshot(2) })
    f.changed({ seq: 3 })
    await settle()
    expect(f.store.getState().snapshot?.revision).toBe(3)
    dispose()
    expect(f.unsubscribe).toHaveBeenCalledTimes(2)
  })

  it('clears on disconnect and refetches reconnect, rejecting late replies from old epochs', async () => {
    const f = fixture()
    const dispose = f.store.getState().subscribe()
    await settle()
    let resolve!: (value: unknown) => void
    f.api.getHostUi.mockImplementationOnce(
      () =>
        new Promise((r) => {
          resolve = r
        })
    )
    f.changed({ seq: 2 })
    f.connection('disconnected')
    expect(f.store.getState().snapshot).toBeNull()
    resolve({ success: true, data: f.snapshot(10) })
    await settle()
    expect(f.store.getState().snapshot).toBeNull()
    f.api.getHostUi.mockResolvedValueOnce({ success: true, data: f.snapshot(1, 'new-boot') })
    f.connection('connected', 'new-boot')
    await settle()
    expect(f.store.getState().snapshot?.bootId).toBe('new-boot')
    dispose()
  })

  it('ignores delayed events and connection messages from retired boots', async () => {
    const f = fixture()
    const dispose = f.store.getState().subscribe()
    await settle()
    f.api.getHostUi.mockResolvedValue({ success: true, data: f.snapshot(1, 'new-boot') })
    f.connection('connected', 'new-boot')
    await settle()
    expect(f.store.getState().snapshot?.bootId).toBe('new-boot')
    f.changed({ bootId: 'boot', seq: 99 })
    f.connection('connected', 'boot')
    await settle()
    expect(f.store.getState().snapshot?.bootId).toBe('new-boot')
    dispose()
  })

  it('preserves accepted context for redundant same-boot connected events', async () => {
    const f = fixture()
    const dispose = f.store.getState().subscribe()
    await settle()
    const context = {
      activeProject: null,
      projectIds: [],
      terminals: [],
      agentSessions: [],
      activeTerminalId: null,
      activeSessionId: null,
      focus: { textInput: false, terminal: false },
      truncated: false
    }
    f.store.getState().acknowledgeContext(4, context)
    f.connection('connected')
    await settle()
    expect(f.store.getState().contextRevision).toBe(4)
    expect(f.store.getState().context).toEqual(context)
    expect(f.store.getState().contextPending).toBe(false)
    dispose()
  })

  it('does not fetch on an explicit initial disconnected event before readiness', async () => {
    let ready!: () => void
    const f = fixture({
      ready: new Promise<void>((resolve) => {
        ready = resolve
      })
    })
    const dispose = f.store.getState().subscribe()
    f.connection('disconnected', null)
    ready()
    await settle()
    expect(f.api.getHostUi).not.toHaveBeenCalled()
    dispose()
  })

  it('owns one listener lifetime across subscribers and cancels teardown before readiness', async () => {
    let ready!: () => void
    const f = fixture({
      ready: new Promise<void>((resolve) => {
        ready = resolve
      })
    })
    const first = f.store.getState().subscribe()
    const second = f.store.getState().subscribe()
    expect(f.api.onChanged).toHaveBeenCalledTimes(1)
    first()
    first()
    expect(f.unsubscribe).not.toHaveBeenCalled()
    second()
    ready()
    await settle()
    expect(f.api.getHostUi).not.toHaveBeenCalled()
    expect(f.unsubscribe).toHaveBeenCalledTimes(2)
  })

  it('caches only the bounded context accepted by the native host and clears it on disconnect', async () => {
    const f = fixture()
    const dispose = f.store.getState().subscribe()
    await settle()
    const context = {
      activeProject: null,
      projectIds: [],
      terminals: [],
      agentSessions: [],
      activeTerminalId: null,
      activeSessionId: null,
      focus: { textInput: false, terminal: false },
      truncated: false
    }
    f.store.getState().acknowledgeContext(4, context)
    expect(f.store.getState().context).toEqual(context)
    expect(f.store.getState().contextPending).toBe(false)
    f.store.getState().acknowledgeContext(3, { ...context, truncated: true })
    expect(f.store.getState().contextRevision).toBe(4)
    expect(f.store.getState().context?.truncated).toBe(false)
    f.connection('disconnected')
    expect(f.store.getState().context).toBeNull()
    expect(f.store.getState().contextPending).toBe(true)
    dispose()
  })

  it('browser mode has zero native listeners or invokes', async () => {
    const f = fixture({ desktop: false })
    f.store.getState().subscribe()()
    await f.store.getState().load()
    expect(f.api.onChanged).not.toHaveBeenCalled()
    expect(f.api.onConnectionChanged).not.toHaveBeenCalled()
    expect(f.api.getHostUi).not.toHaveBeenCalled()
  })

  it('delegates declared execution and dismissal without locally fabricating host records', async () => {
    const f = fixture()
    const dispose = f.store.getState().subscribe()
    await settle()
    const request = {
      pluginId: 'p',
      generation: 'g',
      kind: 'status' as const,
      contributionId: 's',
      contextRevision: 1,
      target: {}
    }
    await f.store.getState().execute(request)
    expect(f.api.executeContribution).toHaveBeenCalledWith(request)
    const dismissal = { pluginId: 'p', generation: 'g', kind: 'notification' as const, id: 'n' }
    await f.store.getState().dismiss(dismissal)
    expect(f.api.dismissHostUi).toHaveBeenCalledWith(dismissal)
    expect(f.api.getHostUi).toHaveBeenCalledTimes(2)
    dispose()
  })
})
