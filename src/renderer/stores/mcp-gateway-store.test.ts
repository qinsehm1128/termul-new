import type { IpcResult } from '@shared/types/ipc.types'
import type { McpGatewaySource } from '@shared/types/mcp-gateway.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  McpClient,
  McpGatewayConnection,
  McpGatewayMode,
  McpGatewayView
} from '@/lib/mcp-gateway-api'
import { MCP_GATEWAY_MODES_KEY, useMcpGatewayStore } from './mcp-gateway-store'

const mocks = vi.hoisted(() => ({
  read: vi.fn(),
  write: vi.fn(),
  view: vi.fn(),
  connection: vi.fn(),
  clients: vi.fn(),
  start: vi.fn(),
  restart: vi.fn(),
  setPort: vi.fn(),
  syncClient: vi.fn(),
  unsyncClient: vi.fn()
}))
vi.mock('@/lib/api', () => ({ persistenceApi: { read: mocks.read, write: mocks.write } }))
vi.mock('@/lib/mcp-gateway-api', () => ({
  MCP_GATEWAY_MODES: ['grouped', 'entry', 'direct'],
  mcpGatewayApi: mocks
}))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
const ok = <T>(data: T): IpcResult<T> => ({ success: true, data })
const view = (
  source: McpGatewaySource,
  port = source === 'internal' ? 3290 : 3390
): McpGatewayView => ({
  source,
  state: 'running',
  port,
  settingsPath: '/temporary/gateway.json'
})
const connection = (
  source: McpGatewaySource,
  mode: McpGatewayMode = 'grouped'
): McpGatewayConnection => ({
  source,
  mode,
  httpReady: true,
  bridgeAvailable: true,
  bridge: { command: '/se-mcp', args: ['--source', source, '--mode', mode] },
  url: 'http://127.0.0.1/mcp',
  token: 'TOKEN_SENTINEL',
  port: source === 'internal' ? 3290 : 3390,
  settingsPath: '/temporary/gateway.json'
})
const runtime = (
  source: McpGatewaySource,
  port = source === 'internal' ? 3290 : 3390,
  pid = 1
): McpGatewayView => ({
  ...view(source, port),
  status: {
    source,
    protocol: 'se-mcp-gateway',
    protocolVersion: 1,
    version: 'test',
    pid,
    port,
    startedAt: pid,
    configRevision: 1,
    configError: null,
    upstreams: [],
    builtIns: []
  }
})

const client = (name: string): McpClient => ({
  id: name,
  name,
  configPath: '/config',
  installed: true,
  synced: true,
  upToDate: true
})
const state = () => useMcpGatewayStore.getState()

beforeEach(() => {
  vi.clearAllMocks()
  mocks.read.mockResolvedValue({ success: true, data: null })
  mocks.write.mockResolvedValue({ success: true })
  mocks.view.mockImplementation(async (source) => ok(view(source)))
  mocks.connection.mockImplementation(async (source, mode) => ok(connection(source, mode)))
  mocks.clients.mockImplementation(async (source) => ok([client(source)]))
  for (const method of ['start', 'restart', 'setPort', 'syncClient', 'unsyncClient'] as const) {
    mocks[method].mockResolvedValue(ok({}))
  }
  const empty = () => ({
    mode: 'grouped' as const,
    view: null,
    connection: null,
    clients: [],
    busy: null,
    error: null
  })
  useMcpGatewayStore.setState({ sources: { internal: empty(), aggregator: empty() } })
})

describe('source-scoped MCP gateway state', () => {
  it('migrates the legacy mode to internal only and persists both modes under the new key', async () => {
    mocks.read.mockImplementation(async (key) => ok(key === 'mcp/gateway-mode' ? 'entry' : null))
    await state().loadMode()
    expect(state().sources.internal.mode).toBe('entry')
    expect(state().sources.aggregator.mode).toBe('grouped')
    expect(mocks.write).toHaveBeenCalledWith(MCP_GATEWAY_MODES_KEY, {
      internal: 'entry',
      aggregator: 'grouped'
    })
    expect(mocks.start).not.toHaveBeenCalled()
    expect(mocks.syncClient).not.toHaveBeenCalled()
  })
  it('prefers the new per-source modes and does not read legacy state', async () => {
    mocks.read.mockResolvedValue(ok({ internal: 'direct', aggregator: 'entry' }))
    await state().loadMode()
    expect(state().sources.internal.mode).toBe('direct')
    expect(state().sources.aggregator.mode).toBe('entry')
    expect(mocks.read).toHaveBeenCalledTimes(1)
  })
  it('does not let a late preference read undo a user mode selection', async () => {
    const pending = deferred<IpcResult<unknown>>()
    mocks.read.mockReturnValue(pending.promise)
    const loading = state().loadMode()
    const changing = state().setMode('aggregator', 'direct')
    expect(state().sources.aggregator.mode).toBe('direct')
    pending.resolve(ok({ internal: 'entry', aggregator: 'entry' }))
    await Promise.all([loading, changing])
    expect(state().sources.aggregator.mode).toBe('direct')
    expect(state().sources.internal.mode).toBe('entry')
    expect(mocks.write).toHaveBeenLastCalledWith(MCP_GATEWAY_MODES_KEY, {
      internal: 'entry',
      aggregator: 'direct'
    })
  })
  it('serializes mode writes and preserves both source choices', async () => {
    const pending = deferred<{ success: boolean }>()
    mocks.write.mockReturnValueOnce(pending.promise)
    const first = state().setMode('internal', 'direct')
    await vi.waitFor(() => expect(mocks.write).toHaveBeenCalledTimes(1))
    const second = state().setMode('aggregator', 'entry')
    pending.resolve({ success: true })
    await Promise.all([first, second])
    expect(mocks.write).toHaveBeenLastCalledWith(MCP_GATEWAY_MODES_KEY, {
      internal: 'direct',
      aggregator: 'entry'
    })
    expect(state().sources.internal.mode).toBe('direct')
    expect(state().sources.aggregator.mode).toBe('entry')
  })
  it('discards late responses for old modes without touching the other source', async () => {
    const pending = deferred<IpcResult<McpGatewayConnection>>()
    mocks.connection.mockImplementation((source, mode) =>
      source === 'internal' && mode === 'grouped'
        ? pending.promise
        : Promise.resolve(ok(connection(source, mode)))
    )
    const old = state().refresh('internal')
    await state().refresh('aggregator')
    const other = state().sources.aggregator
    await state().setMode('internal', 'direct')
    pending.resolve(ok(connection('internal', 'grouped')))
    await old
    expect(state().sources.internal.mode).toBe('direct')
    expect(state().sources.internal.connection?.mode).toBe('direct')
    expect(state().sources.aggregator).toBe(other)
  })
  it('rejects cross-source connection/view responses instead of copying them', async () => {
    mocks.view.mockResolvedValue(ok(view('aggregator')))
    mocks.connection.mockResolvedValue(ok(connection('aggregator')))
    await state().refresh('internal')
    expect(state().sources.internal.view).toBeNull()
    expect(state().sources.internal.connection).toBeNull()
    expect(state().sources.internal.error).toBe('MCP_GATEWAY_REFRESH_FAILED')
  })
  it.each([
    { source: 'aggregator', protocol: 'se-mcp-gateway', protocolVersion: 1 },
    { source: 'internal', protocol: 'legacy-mixed', protocolVersion: 1 },
    { source: 'internal', protocol: 'se-mcp-gateway', protocolVersion: 2 },
    { source: undefined, protocol: undefined, protocolVersion: undefined }
  ])('rejects mismatched or missing status identity %j', async (identity) => {
    const candidate = {
      ...view('internal'),
      status: {
        ...identity,
        version: '1',
        pid: 1,
        port: 3290,
        startedAt: 0,
        configRevision: null,
        configError: null,
        upstreams: [],
        builtIns: []
      }
    } as McpGatewayView
    mocks.view.mockResolvedValue(ok(candidate))
    await state().refresh('internal')
    expect(state().sources.internal.view).toBeNull()
    await state().refreshView('internal')
    expect(state().sources.internal.view).toBeNull()
    expect(state().sources.internal.error).toBe('MCP_GATEWAY_REFRESH_FAILED')
  })
  it('keeps both syncs busy independently, and one failure cannot clear the other action', async () => {
    const internal = deferred<IpcResult<unknown>>()
    const aggregator = deferred<IpcResult<unknown>>()
    mocks.syncClient.mockImplementation((source) =>
      source === 'internal' ? internal.promise : aggregator.promise
    )
    await state().setMode('aggregator', 'entry')
    const first = state().syncClient('internal', 'codex')
    const second = state().syncClient('aggregator', 'codex')
    expect(mocks.syncClient.mock.calls).toEqual([
      ['internal', 'codex', 'grouped'],
      ['aggregator', 'codex', 'entry']
    ])
    expect(state().sources.internal.busy).toBe('sync:codex')
    expect(state().sources.aggregator.busy).toBe('sync:codex')
    internal.resolve({ success: false, error: 'RAW_ERROR TOKEN_SENTINEL' })
    expect(await first).toBe(false)
    expect(state().sources.internal.busy).toBeNull()
    expect(state().sources.internal.error).toBe('MCP_GATEWAY_ACTION_FAILED')
    expect(state().sources.aggregator.busy).toBe('sync:codex')
    aggregator.resolve(ok({}))
    expect(await second).toBe(true)
    expect(state().sources.aggregator.busy).toBeNull()
    expect(state().sources.aggregator.error).toBeNull()
    expect(JSON.stringify(state().sources.internal)).not.toContain('TOKEN_SENTINEL')
  })
  it('scopes start, restart, port and unsync to only the selected source', async () => {
    await state().refresh('internal')
    const untouched = state().sources.internal
    expect(await state().start('aggregator')).toBe(true)
    expect(await state().restart('aggregator')).toBe(true)
    expect(await state().setPort('aggregator', 4000)).toBe(true)
    expect(await state().unsyncClient('aggregator', 'cursor')).toBe(true)
    expect(mocks.start).toHaveBeenCalledWith('aggregator')
    expect(mocks.restart).toHaveBeenCalledWith('aggregator')
    expect(mocks.setPort).toHaveBeenCalledWith('aggregator', 4000)
    expect(mocks.unsyncClient).toHaveBeenCalledWith('aggregator', 'cursor')
    expect(state().sources.internal).toBe(untouched)
  })
  it('contains rejected requests and preserves the healthy side', async () => {
    mocks.view.mockImplementation((source) =>
      source === 'internal'
        ? Promise.reject(new Error('TOKEN_SENTINEL'))
        : Promise.resolve(ok(view(source)))
    )
    await Promise.all([state().refresh('internal'), state().refresh('aggregator')])
    expect(state().sources.internal.error).toBe('MCP_GATEWAY_REFRESH_FAILED')
    expect(state().sources.aggregator.view?.source).toBe('aggregator')
    expect(state().sources.aggregator.error).toBeNull()
  })
  it('rejects a late status poll without overwriting a newer view', async () => {
    const pending = deferred<IpcResult<McpGatewayView>>()
    mocks.view.mockReturnValueOnce(pending.promise)
    const old = state().refreshView('internal')
    mocks.view.mockResolvedValueOnce(ok(view('internal', 4000)))
    await state().refreshView('internal')
    pending.resolve(ok(view('internal', 3290)))
    await old
    expect(state().sources.internal.view?.port).toBe(4000)
  })
  it('invalidates an old-mode refresh when persisted modes finish loading', async () => {
    const pending = deferred<IpcResult<McpGatewayConnection>>()
    mocks.connection.mockReturnValueOnce(pending.promise)
    const refresh = state().refresh('internal')
    mocks.read.mockResolvedValue(ok({ internal: 'entry', aggregator: 'grouped' }))
    await state().loadMode()
    pending.resolve(ok(connection('internal', 'grouped')))
    await refresh
    expect(state().sources.internal.mode).toBe('entry')
    expect(state().sources.internal.connection).toBeNull()
    expect(state().sources.internal.clients).toEqual([])
  })
  it('ignores a superseded view failure within a full refresh', async () => {
    const pending = deferred<IpcResult<McpGatewayView>>()
    mocks.view.mockReturnValueOnce(pending.promise)
    const refresh = state().refresh('internal')
    await state().refreshView('internal')
    pending.resolve({ success: false, error: 'TOKEN_SENTINEL' })
    await refresh
    expect(state().sources.internal.view?.state).toBe('running')
    expect(state().sources.internal.error).toBeNull()
  })
  it('clears a refresh failure after a successful retry', async () => {
    mocks.connection.mockResolvedValueOnce({ success: false })
    await state().refresh('internal')
    expect(state().sources.internal.error).toBe('MCP_GATEWAY_REFRESH_FAILED')
    await state().refresh('internal')
    expect(state().sources.internal.error).toBeNull()
  })
  it('reports current preference write failures without exposing raw error text', async () => {
    mocks.write.mockResolvedValueOnce({ success: false, error: 'TOKEN_SENTINEL' })
    await state().setMode('internal', 'entry')
    expect(state().sources.internal.error).toBe('MCP_GATEWAY_PREFERENCE_FAILED')
    expect(state().sources.aggregator.error).toBeNull()
  })
  it('does not attach a stale preference write failure to a newer mode choice', async () => {
    const pending = deferred<{ success: boolean }>()
    mocks.write.mockReturnValueOnce(pending.promise)
    const first = state().setMode('internal', 'entry')
    await vi.waitFor(() => expect(mocks.write).toHaveBeenCalledTimes(1))
    const second = state().setMode('internal', 'direct')
    pending.resolve({ success: false })
    await Promise.all([first, second])
    expect(state().sources.internal.mode).toBe('direct')
    expect(state().sources.internal.error).toBeNull()
  })
  it('catches action exceptions without keeping a source busy or exposing raw errors', async () => {
    mocks.start.mockRejectedValue(new Error('TOKEN_SENTINEL'))
    expect(await state().start('internal')).toBe(false)
    expect(state().sources.internal.busy).toBeNull()
    expect(state().sources.internal.error).toBe('MCP_GATEWAY_ACTION_FAILED')
  })
})

const pollFailures = [
  'stopped',
  'error',
  'wrong-source',
  'wrong-protocol',
  'api-failure',
  'exception'
] as const

describe.each(['internal', 'aggregator'] as const)('%s HTTP polling lifecycle', (source) => {
  const other: McpGatewaySource = source === 'internal' ? 'aggregator' : 'internal'
  it.each(
    pollFailures
  )('revokes ready HTTP on %s, preserves offline bridge and revalidates on recovery', async (failure) => {
    await state().refresh(source)
    await state().setMode(other, 'entry')
    const untouched = state().sources[other]
    const bridge = state().sources[source].connection?.bridge
    const calls = mocks.connection.mock.calls.length
    if (failure === 'exception') mocks.view.mockRejectedValueOnce(new Error('TOKEN_SENTINEL'))
    else if (failure === 'api-failure')
      mocks.view.mockResolvedValueOnce({ success: false, error: 'TOKEN_SENTINEL' })
    else if (failure === 'wrong-source') mocks.view.mockResolvedValueOnce(ok(view(other)))
    else if (failure === 'wrong-protocol') {
      const bad = runtime(source)
      if (bad.status) bad.status.protocol = 'legacy-mixed'
      mocks.view.mockResolvedValueOnce(ok(bad))
    } else mocks.view.mockResolvedValueOnce(ok({ ...view(source), state: failure }))
    await state().refreshView(source)
    expect(state().sources[source].connection?.httpReady).toBe(false)
    expect(state().sources[source].connection?.token).toBe('')
    expect(state().sources[source].connection?.bridge).toBe(bridge)
    expect(mocks.connection).toHaveBeenCalledTimes(calls)
    expect(state().sources[other]).toBe(untouched)
    const pending = deferred<IpcResult<McpGatewayConnection>>()
    mocks.connection.mockReturnValueOnce(pending.promise)
    const recovering = state().refreshView(source)
    await vi.waitFor(() => expect(mocks.connection).toHaveBeenCalledTimes(calls + 1))
    expect(state().sources[source].connection?.httpReady).toBe(false)
    pending.resolve(ok({ ...connection(source), token: 'NEW_VERIFIED_TOKEN' }))
    await recovering
    expect(state().sources[source].connection?.httpReady).toBe(true)
    expect(state().sources[source].connection?.token).toBe('NEW_VERIFIED_TOKEN')
    expect(state().sources[source].error).toBeNull()
    expect(state().sources[other]).toBe(untouched)
    expect(state().sources[other].mode).toBe('entry')
  })

  it('revalidates an initially unready connection when the runtime recovers', async () => {
    mocks.connection.mockResolvedValueOnce(ok({ ...connection(source), httpReady: false }))
    await state().refresh(source)
    await state().refresh(other)
    const untouched = state().sources[other]
    expect(state().sources[source].connection?.httpReady).toBe(false)
    await state().refreshView(source)
    expect(state().sources[source].connection?.httpReady).toBe(true)
    expect(mocks.connection).toHaveBeenLastCalledWith(source, 'grouped')
    expect(state().sources[other]).toBe(untouched)
  })

  it.each([
    'port',
    'identity'
  ] as const)('revokes readiness during a %s change until the new connection verifies', async (change) => {
    mocks.view.mockImplementation(async (selected) => ok(runtime(selected)))
    await state().refresh(source)
    await state().setMode(other, 'direct')
    const untouched = state().sources[other]
    const port = change === 'port' ? 4400 : connection(source).port
    mocks.view.mockResolvedValueOnce(ok(runtime(source, port, change === 'identity' ? 2 : 1)))
    const pending = deferred<IpcResult<McpGatewayConnection>>()
    mocks.connection.mockReturnValueOnce(pending.promise)
    const poll = state().refreshView(source)
    await vi.waitFor(() => expect(state().sources[source].connection?.httpReady).toBe(false))
    pending.resolve(ok({ ...connection(source), port, token: 'REPLACEMENT_VERIFIED_TOKEN' }))
    await poll
    expect(state().sources[source].connection).toMatchObject({
      httpReady: true,
      port,
      token: 'REPLACEMENT_VERIFIED_TOKEN'
    })
    expect(state().sources[other]).toBe(untouched)
  })

  it('does not re-fetch an unchanged healthy connection on every status poll', async () => {
    await state().refresh(source)
    const original = state().sources[source].connection
    const calls = mocks.connection.mock.calls.length
    await state().refreshView(source)
    await state().refreshView(source)
    expect(state().sources[source].connection).toBe(original)
    expect(mocks.connection).toHaveBeenCalledTimes(calls)
  })

  it('cannot revive a delayed verification result after a newer stopped poll', async () => {
    mocks.connection.mockResolvedValueOnce(ok({ ...connection(source), httpReady: false }))
    await state().refresh(source)
    const pending = deferred<IpcResult<McpGatewayConnection>>()
    mocks.connection.mockReturnValueOnce(pending.promise)
    const old = state().refreshView(source)
    await vi.waitFor(() => expect(mocks.connection).toHaveBeenCalledTimes(2))
    mocks.view.mockResolvedValueOnce(ok({ ...view(source), state: 'stopped' }))
    await state().refreshView(source)
    pending.resolve(ok(connection(source)))
    await old
    expect(state().sources[source].view?.state).toBe('stopped')
    expect(state().sources[source].connection?.httpReady).toBe(false)
    expect(state().sources[source].connection?.token).toBe('')
  })

  it('cannot revive a delayed full refresh after a newer failed poll', async () => {
    await state().refresh(source)
    const pending = deferred<IpcResult<McpGatewayConnection>>()
    mocks.connection.mockReturnValueOnce(pending.promise)
    const old = state().refresh(source)
    mocks.view.mockResolvedValueOnce({ success: false })
    await state().refreshView(source)
    pending.resolve(ok(connection(source)))
    await old
    expect(state().sources[source].view).toBeNull()
    expect(state().sources[source].connection?.httpReady).toBe(false)
    expect(state().sources[source].error).toBe('MCP_GATEWAY_POLL_FAILED')
  })

  it('does not let old-mode verification overwrite a newer mode', async () => {
    mocks.connection.mockResolvedValueOnce(ok({ ...connection(source), httpReady: false }))
    await state().refresh(source)
    await state().setMode(other, 'entry')
    const untouched = state().sources[other]
    const pending = deferred<IpcResult<McpGatewayConnection>>()
    mocks.connection.mockReturnValueOnce(pending.promise)
    const old = state().refreshView(source)
    await vi.waitFor(() => expect(state().sources[source].connection?.token).toBe(''))
    await state().setMode(source, 'direct')
    pending.resolve(ok(connection(source, 'grouped')))
    await old
    expect(state().sources[source].mode).toBe('direct')
    expect(state().sources[source].connection).toMatchObject({ mode: 'direct', httpReady: true })
    expect(state().sources[other]).toBe(untouched)
  })

  it.each([
    'source',
    'mode',
    'port'
  ] as const)('rejects a revalidation response with the wrong %s', async (field) => {
    mocks.connection.mockResolvedValueOnce(ok({ ...connection(source), httpReady: false }))
    await state().refresh(source)
    const candidate = {
      ...connection(source),
      [field]: field === 'source' ? other : field === 'mode' ? 'entry' : 9999
    }
    mocks.connection.mockResolvedValueOnce(ok(candidate))
    await state().refreshView(source)
    expect(state().sources[source].connection?.httpReady).toBe(false)
    expect(state().sources[source].connection?.token).toBe('')
    expect(state().sources[source].error).toBe('MCP_GATEWAY_POLL_FAILED')
  })

  it.each([
    'MCP_GATEWAY_ACTION_FAILED',
    'MCP_GATEWAY_PREFERENCE_FAILED',
    'MCP_GATEWAY_REFRESH_FAILED'
  ])('does not erase %s during failed and recovered polls', async (error) => {
    await state().refresh(source)
    useMcpGatewayStore.setState((current) => ({
      sources: { ...current.sources, [source]: { ...current.sources[source], error } }
    }))
    mocks.view.mockResolvedValueOnce({ success: false })
    await state().refreshView(source)
    expect(state().sources[source].error).toBe(error)
    await state().refreshView(source)
    expect(state().sources[source].error).toBe(error)
    expect(state().sources[source].connection?.httpReady).toBe(true)
  })
})
