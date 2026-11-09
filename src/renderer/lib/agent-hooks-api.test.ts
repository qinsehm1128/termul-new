import type { AdapterStatusDto, HookStateSyncResponse } from '@shared/types/agent-hooks.types'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  agentHooksApi,
  createHookStateSubscription,
  type HookStateListener,
  type HookStateTransport
} from './agent-hooks-api'

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  desktop: true,
  warn: vi.fn(),
  onEvent: vi.fn(),
  connect: vi.fn(),
  subscribeConnection: vi.fn(),
  getTerminalState: vi.fn()
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }))
vi.mock('./tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('./log-api', () => ({ logFrontendError: mocks.warn }))
vi.mock('@/features/agent-session/stores/acp-store', () => ({
  useAcpStore: { subscribe: mocks.subscribeConnection }
}))
vi.mock('./acp-transport', () => ({
  getAcpTransport: () => ({
    onEvent: mocks.onEvent,
    connect: mocks.connect,
    subscribeConnection: mocks.subscribeConnection,
    getTerminalState: mocks.getTerminalState
  })
}))

const statuses: AdapterStatusDto[] = [
  {
    id: 'pi',
    display_name: 'Pi',
    version: '3.0.0',
    origin: 'builtin',
    state: 'installed',
    installed_version: '3.0.0',
    install_id: 'install',
    capabilities: { chat: true, resume: true, events: [] },
    diagnostics: []
  }
]
const preview = { adapter_id: 'pi', action: 'install' as const, event_options: {} }
const confirm = { adapter_id: 'pi', preview_id: 'preview', fingerprint: 'sha256' }
const snapshot = (revision = 1): HookStateSyncResponse => ({
  kind: 'snapshot',
  instance_id: 'host',
  current_revision: revision,
  states: [],
  tombstones: []
})
const cleanups: (() => void)[] = []
const flush = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve()
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.desktop = true
  mocks.getTerminalState.mockReturnValue(null)
  Object.defineProperty(document, 'hidden', { configurable: true, value: false })
})
afterEach(() => {
  for (const stop of cleanups.splice(0)) stop()
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('hook adapter API runtime parity', () => {
  it('invokes the frozen command names with request DTOs', async () => {
    mocks.invoke.mockResolvedValue(statuses)
    expect(await agentHooksApi.status()).toEqual(statuses)
    await agentHooksApi.reload()
    await agentHooksApi.probe('pi')
    await agentHooksApi.preview(preview)
    await agentHooksApi.install(confirm)
    await agentHooksApi.uninstall(confirm)
    await agentHooksApi.confirm(confirm)
    await agentHooksApi.openFolder()
    await agentHooksApi.state({ instance_id: 'host', since_revision: 7 })
    expect(mocks.invoke.mock.calls).toEqual([
      ['agent_hooks_status_cmd'],
      ['agent_hooks_reload_cmd'],
      ['agent_hooks_probe_cmd', { adapterId: 'pi' }],
      ['agent_hooks_preview_cmd', { request: preview }],
      ['agent_hooks_install_cmd', { request: confirm }],
      ['agent_hooks_uninstall_cmd', { request: confirm }],
      ['agent_hooks_confirm_cmd', { request: confirm }],
      ['agent_hooks_open_folder_cmd'],
      ['agent_hooks_state_cmd', { request: { instance_id: 'host', since_revision: 7 } }]
    ])
  })

  it('uses exact web routes, snake_case bodies and revision query', async () => {
    mocks.desktop = false
    const fetcher = vi
      .fn()
      .mockImplementation(() =>
        Promise.resolve(
          new Response(JSON.stringify({ success: true, data: statuses }), { status: 200 })
        )
      )
    vi.stubGlobal('fetch', fetcher)
    expect(await agentHooksApi.status()).toEqual(statuses)
    await agentHooksApi.reload()
    await agentHooksApi.probe('pi')
    await agentHooksApi.preview(preview)
    await agentHooksApi.install(confirm)
    await agentHooksApi.uninstall(confirm)
    await agentHooksApi.confirm(confirm)
    await agentHooksApi.state({ instance_id: 'host with spaces', since_revision: 0 })
    expect(
      fetcher.mock.calls.map(([url]) => String(url).replace(window.location.origin, ''))
    ).toEqual([
      '/api/agent-hooks/status',
      '/api/agent-hooks/reload',
      '/api/agent-hooks/probe',
      '/api/agent-hooks/preview',
      '/api/agent-hooks/install',
      '/api/agent-hooks/uninstall',
      '/api/agent-hooks/confirm',
      '/api/agent-hooks/state?instance_id=host+with+spaces&since_revision=0'
    ])
    expect(JSON.parse(fetcher.mock.calls[3][1].body)).toEqual(preview)
    expect(JSON.parse(fetcher.mock.calls[4][1].body)).toEqual(confirm)
    expect(fetcher.mock.calls[3][1].method).toBe('POST')
  })

  it('transports trusted two-step resume requests without executable input', async () => {
    const plan = { descriptor_id: 'descriptor', project_id: 'project', expected_generation: 7 }
    const spawn = { plan_id: 'plan', descriptor_id: 'descriptor', expected_generation: 7 }
    mocks.invoke.mockResolvedValue({})
    await agentHooksApi.resumePlan(plan)
    await agentHooksApi.resumeSpawn(spawn)
    expect(mocks.invoke.mock.calls).toEqual([
      ['agent_hooks_resume_plan_cmd', { request: plan }],
      ['agent_hooks_resume_spawn_cmd', { request: spawn }]
    ])
    mocks.desktop = false
    const fetcher = vi
      .fn()
      .mockImplementation(async () => new Response(JSON.stringify({ success: true, data: {} })))
    vi.stubGlobal('fetch', fetcher)
    await agentHooksApi.resumePlan(plan)
    await agentHooksApi.resumeSpawn(spawn)
    expect(
      fetcher.mock.calls.map(([url]) => String(url).replace(window.location.origin, ''))
    ).toEqual(['/api/agent-hooks/resume-plan', '/api/agent-hooks/resume-spawn'])
    expect(fetcher.mock.calls.map(([, init]) => JSON.parse(init.body))).toEqual([plan, spawn])
  })

  it('returns UNSUPPORTED_PLATFORM for web open folder without making a request', async () => {
    mocks.desktop = false
    const fetcher = vi.fn()
    vi.stubGlobal('fetch', fetcher)
    await expect(agentHooksApi.openFolder()).rejects.toMatchObject({ code: 'UNSUPPORTED_PLATFORM' })
    expect(fetcher).not.toHaveBeenCalled()
  })

  it('preserves conflict and authorization error codes', async () => {
    mocks.desktop = false
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            success: false,
            error: 'Preview changed',
            code: 'CONFLICT'
          }),
          { status: 409 }
        )
      )
    )
    await expect(agentHooksApi.install(confirm)).rejects.toMatchObject({ code: 'CONFLICT' })
  })
})

describe('shared push connector', () => {
  it('subscribes to agent-hooks-state in Tauri and releases the listener', async () => {
    vi.useFakeTimers()
    const stop = vi.fn()
    mocks.listen.mockResolvedValue(stop)
    mocks.invoke.mockResolvedValue(snapshot())
    const listener = vi.fn()
    const unsubscribe = agentHooksApi.subscribeState(listener)
    cleanups.push(unsubscribe)
    await vi.dynamicImportSettled()
    await flush()
    expect(mocks.listen).toHaveBeenCalledWith('agent-hooks-state', expect.any(Function))
    const baseline = mocks.invoke.mock.calls.length
    expect(vi.getTimerCount()).toBe(0)
    const handler = mocks.listen.mock.calls[0][1]
    handler({ payload: { ...snapshot(4), kind: 'delta' } })
    expect(listener).toHaveBeenLastCalledWith({ ...snapshot(4), kind: 'delta' })
    await vi.advanceTimersByTimeAsync(60000)
    expect(mocks.invoke).toHaveBeenCalledTimes(baseline)
    unsubscribe()
    expect(stop).toHaveBeenCalledOnce()
  })

  it('uses shared authenticated web push and observes published disconnect/reconnect state', async () => {
    vi.useFakeTimers()
    mocks.desktop = false
    const unlisten = vi.fn()
    const unsubscribeConnection = vi.fn()
    mocks.onEvent.mockReturnValue(unlisten)
    mocks.subscribeConnection.mockReturnValue(unsubscribeConnection)
    mocks.connect.mockResolvedValue(undefined)
    const fetcher = vi
      .fn()
      .mockImplementation(
        async () => new Response(JSON.stringify({ success: true, data: snapshot() }))
      )
    vi.stubGlobal('fetch', fetcher)
    const unsubscribe = agentHooksApi.subscribeState(vi.fn())
    cleanups.push(unsubscribe)
    await vi.dynamicImportSettled()
    await flush()
    expect(mocks.onEvent).toHaveBeenCalledWith('agent-hooks-state', expect.any(Function))
    expect(vi.getTimerCount()).toBe(0)
    const baseline = fetcher.mock.calls.length
    await vi.advanceTimersByTimeAsync(60000)
    expect(fetcher).toHaveBeenCalledTimes(baseline)
    const connection = mocks.subscribeConnection.mock.calls[0][0]
    connection({ transportReconnecting: true }, { transportReconnecting: false })
    expect(vi.getTimerCount()).toBe(1)
    await vi.advanceTimersByTimeAsync(1000)
    expect(fetcher).toHaveBeenCalledTimes(baseline + 1)
    connection({ transportReconnecting: false }, { transportReconnecting: true })
    await flush()
    expect(vi.getTimerCount()).toBe(0)
    mocks.getTerminalState.mockReturnValue({ code: 'REAUTHENTICATION_REQUIRED' })
    connection({ transportReconnecting: false }, { transportReconnecting: true })
    expect(vi.getTimerCount()).toBe(1)
    const revocation = mocks.onEvent.mock.calls.find(
      ([type]) => type === 'reauthentication_required'
    )?.[1]
    revocation?.({})
    expect(vi.getTimerCount()).toBe(1)
    unsubscribe()
    expect(unlisten).toHaveBeenCalledTimes(2)
    expect(unsubscribeConnection).toHaveBeenCalledOnce()
  })
})

function harness() {
  let onConnection: (connected: boolean) => void = () => {}
  let onPush: HookStateListener = () => {}
  const stop = vi.fn()
  const state = vi.fn<HookStateTransport['state']>().mockResolvedValue(snapshot())
  const connect = vi
    .fn<HookStateTransport['connect']>()
    .mockImplementation(async (push, connection) => {
      onPush = push
      onConnection = connection
      return stop
    })
  const subscribe = createHookStateSubscription({ state, connect })
  const listener = vi.fn<HookStateListener>()
  cleanups.push(subscribe(listener))
  return {
    state,
    connect,
    stop,
    subscribe,
    listener,
    connection: (next: boolean) => onConnection(next),
    push: (response: HookStateSyncResponse) => onPush(response)
  }
}

describe('singleton revision reconciliation', () => {
  beforeEach(() => vi.useFakeTimers())

  it('owns only ONE reconcile timer across subscribers and never polls with connected push', async () => {
    const h = harness()
    cleanups.push(h.subscribe(vi.fn()))
    await flush()
    expect(h.connect).toHaveBeenCalledTimes(1)
    expect(vi.getTimerCount()).toBe(1)
    h.connection(true)
    await flush()
    const baseline = h.state.mock.calls.length
    expect(vi.getTimerCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(120000)
    expect(h.state).toHaveBeenCalledTimes(baseline)
    h.connection(false)
    h.connection(false)
    expect(vi.getTimerCount()).toBe(1)
  })

  it('backs off exactly 1/2/4/8/16/30 seconds and caps at 30', async () => {
    const h = harness()
    await flush()
    for (const [index, delay] of [1000, 2000, 4000, 8000, 16000, 30000, 30000].entries()) {
      await vi.advanceTimersByTimeAsync(delay - 1)
      expect(h.state).toHaveBeenCalledTimes(index + 1)
      expect(vi.getTimerCount()).toBe(1)
      await vi.advanceTimersByTimeAsync(1)
      expect(h.state).toHaveBeenCalledTimes(index + 2)
      expect(vi.getTimerCount()).toBe(1)
    }
  })

  it('keeps a single request in flight, pauses hidden pages, resumes once visible', async () => {
    const h = harness()
    await flush()
    let resolve!: (value: HookStateSyncResponse) => void
    h.state.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    await vi.advanceTimersByTimeAsync(1000)
    expect(h.state).toHaveBeenCalledTimes(2)
    expect(vi.getTimerCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(120000)
    expect(h.state).toHaveBeenCalledTimes(2)
    Object.defineProperty(document, 'hidden', { configurable: true, value: true })
    document.dispatchEvent(new Event('visibilitychange'))
    resolve(snapshot(2))
    await flush()
    expect(vi.getTimerCount()).toBe(0)
    Object.defineProperty(document, 'hidden', { configurable: true, value: false })
    document.dispatchEvent(new Event('visibilitychange'))
    await flush()
    expect(h.state).toHaveBeenCalledTimes(3)
    expect(vi.getTimerCount()).toBe(1)
  })

  it('advances the cursor from current_revision, ignoring authorized revision gaps', async () => {
    const h = harness()
    await flush()
    h.push({ ...snapshot(83), kind: 'delta' })
    await vi.advanceTimersByTimeAsync(1000)
    expect(h.state).toHaveBeenLastCalledWith({ instance_id: 'host', since_revision: 83 })
    expect(h.listener).toHaveBeenCalledWith({ ...snapshot(83), kind: 'delta' })
  })

  it('buffers pushes during snapshot fetch and ignores stale responses', async () => {
    const h = harness()
    await flush()
    let resolve!: (value: HookStateSyncResponse) => void
    h.state.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    await vi.advanceTimersByTimeAsync(1000)
    h.push({ ...snapshot(5), kind: 'delta' })
    resolve(snapshot(4))
    await flush()
    expect(h.listener.mock.calls.slice(-2).map(([message]) => message.current_revision)).toEqual([
      4, 5
    ])
    h.push({ ...snapshot(3), kind: 'delta' })
    expect(h.listener).toHaveBeenLastCalledWith({ ...snapshot(5), kind: 'delta' })
  })

  it('provides a full cached snapshot for a late consumer, retaining tombstones semantics', async () => {
    const h = harness()
    await flush()
    h.push({
      ...snapshot(2),
      kind: 'delta',
      states: [
        {
          instance_id: 'host',
          terminal_id: 'pty',
          generation: 1,
          adapter_install_id: 'install',
          agent_session_id: 'session',
          project_id: 'project',
          state: 'idle',
          restore_state: 'confirmed',
          revision: 2,
          identity: {
            adapter_id: 'pi',
            agent_session_id: 'session',
            transcript_path: null,
            cwd: null,
            source: 'hook',
            descriptor_id: null,
            resume_argv: null
          }
        }
      ]
    })
    const late = vi.fn()
    cleanups.push(h.subscribe(late))
    expect(late.mock.calls[0][0].states).toHaveLength(1)
    h.push({
      ...snapshot(3),
      kind: 'delta',
      tombstones: [
        {
          instance_id: 'host',
          terminal_id: 'pty',
          generation: 1,
          revision: 3
        }
      ]
    })
    const afterRemoval = vi.fn()
    cleanups.push(h.subscribe(afterRemoval))
    expect(afterRemoval).toHaveBeenCalledWith(snapshot(3))
    h.push({ ...snapshot(0), instance_id: 'new-instance' })
    expect(h.listener).toHaveBeenLastCalledWith({ ...snapshot(0), instance_id: 'new-instance' })
  })

  it('cleans up the only timer and push subscription on final unsubscribe', async () => {
    const h = harness()
    await flush()
    for (const stop of cleanups.splice(0)) stop()
    expect(vi.getTimerCount()).toBe(0)
    expect(h.stop).toHaveBeenCalledOnce()
    await vi.advanceTimersByTimeAsync(120000)
    expect(h.state).toHaveBeenCalledTimes(1)
  })

  it('does not reconcile hidden pages and cleans a push setup that resolves after disposal', async () => {
    Object.defineProperty(document, 'hidden', { configurable: true, value: true })
    const state = vi.fn().mockResolvedValue(snapshot())
    let resolve!: (stop: () => void) => void
    const subscribe = createHookStateSubscription({
      state,
      connect: () =>
        new Promise((done) => {
          resolve = done
        })
    })
    const stop = subscribe(vi.fn())
    cleanups.push(stop)
    await flush()
    expect(state).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
    stop()
    const unlisten = vi.fn()
    resolve(unlisten)
    await flush()
    expect(unlisten).toHaveBeenCalledOnce()
    Object.defineProperty(document, 'hidden', { configurable: true, value: false })
    document.dispatchEvent(new Event('visibilitychange'))
    expect(state).not.toHaveBeenCalled()
  })

  it('does not advance the initial cursor from hidden-tab push deltas', async () => {
    Object.defineProperty(document, 'hidden', { configurable: true, value: true })
    const h = harness()
    h.connection(true)
    h.push({ ...snapshot(50), kind: 'delta' })
    await flush()
    expect(h.listener).not.toHaveBeenCalled()
    expect(h.state).not.toHaveBeenCalled()
    Object.defineProperty(document, 'hidden', { configurable: true, value: false })
    document.dispatchEvent(new Event('visibilitychange'))
    await flush()
    expect(h.state).toHaveBeenCalledExactlyOnceWith({})
    expect(h.listener).toHaveBeenCalledExactlyOnceWith(snapshot())
  })

  it('requires a new full baseline for a changed instance, including buffered pushes', async () => {
    const h = harness()
    await flush()
    let resolve!: (value: HookStateSyncResponse) => void
    h.state.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    h.state.mockResolvedValueOnce({ ...snapshot(10), instance_id: 'replacement' })
    await vi.advanceTimersByTimeAsync(1000)
    h.push({ ...snapshot(9), instance_id: 'replacement', kind: 'delta' })
    resolve(snapshot(2))
    await flush()
    expect(h.state).toHaveBeenLastCalledWith({ instance_id: 'host', since_revision: 2 })
    expect(h.listener).toHaveBeenLastCalledWith({ ...snapshot(10), instance_id: 'replacement' })
    expect(
      h.listener.mock.calls.some(
        ([message]) => message.kind === 'delta' && message.instance_id === 'replacement'
      )
    ).toBe(false)
  })

  it('uses fallback after push setup fails and isolates subscriber failures', async () => {
    const state = vi.fn().mockResolvedValue(snapshot())
    const subscribe = createHookStateSubscription({
      state,
      connect: async () => {
        throw new Error('closed')
      }
    })
    cleanups.push(
      subscribe(() => {
        throw new Error('consumer')
      })
    )
    await flush()
    expect(vi.getTimerCount()).toBe(1)
    await vi.advanceTimersByTimeAsync(1000)
    expect(state).toHaveBeenCalledTimes(2)
    expect(mocks.warn).toHaveBeenCalledWith({
      level: 'warn',
      source: 'agent-hooks-api',
      message: '[AgentHooks] state push unavailable'
    })
  })
})
