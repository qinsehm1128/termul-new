import type { McpGatewaySource } from '@shared/types/mcp-gateway.types'
import { create } from 'zustand'
import { persistenceApi } from '@/lib/api'
import {
  MCP_GATEWAY_MODES,
  type McpClient,
  type McpGatewayConnection,
  type McpGatewayMode,
  type McpGatewayView,
  mcpGatewayApi
} from '@/lib/mcp-gateway-api'

/** Legacy preference is read only, and belongs exclusively to internal. */
export const MCP_GATEWAY_MODE_KEY = 'mcp/gateway-mode'
export const MCP_GATEWAY_MODES_KEY = 'mcp/gateway-modes'

export interface McpGatewaySourceState {
  mode: McpGatewayMode
  view: McpGatewayView | null
  connection: McpGatewayConnection | null
  clients: McpClient[]
  busy: string | null
  error: string | null
}

interface McpGatewayState {
  sources: Record<McpGatewaySource, McpGatewaySourceState>
  loadMode: () => Promise<void>
  setMode: (source: McpGatewaySource, mode: McpGatewayMode) => Promise<void>
  refresh: (source: McpGatewaySource) => Promise<void>
  refreshView: (source: McpGatewaySource) => Promise<void>
  start: (source: McpGatewaySource) => Promise<boolean>
  restart: (source: McpGatewaySource) => Promise<boolean>
  setPort: (source: McpGatewaySource, port: number) => Promise<boolean>
  syncClient: (source: McpGatewaySource, id: string) => Promise<boolean>
  unsyncClient: (source: McpGatewaySource, id: string) => Promise<boolean>
}

function isMode(value: unknown): value is McpGatewayMode {
  return MCP_GATEWAY_MODES.includes(value as McpGatewayMode)
}

function viewMatchesSource(view: McpGatewayView, source: McpGatewaySource): boolean {
  return (
    view.source === source &&
    (!view.status ||
      (view.status.source === source &&
        view.status.protocol === 'se-mcp-gateway' &&
        view.status.protocolVersion === 1))
  )
}

function revokeHttp(connection: McpGatewayConnection | null): McpGatewayConnection | null {
  if (!connection || (connection.httpReady === false && !connection.token)) return connection
  // Keep the offline bridge usable, but never retain an exportable stale bearer.
  return { ...connection, httpReady: false, token: '' }
}

function connectionMatchesView(
  connection: McpGatewayConnection,
  view: McpGatewayView | null,
  source: McpGatewaySource,
  mode: McpGatewayMode
): boolean {
  return (
    connection.source === source &&
    connection.mode === mode &&
    !!view &&
    viewMatchesSource(view, source) &&
    view.state === 'running' &&
    view.port === connection.port
  )
}

function sameRuntime(previous: McpGatewayView | null, next: McpGatewayView): boolean {
  return (
    !!previous &&
    previous.state === 'running' &&
    previous.source === next.source &&
    previous.port === next.port &&
    previous.settingsPath === next.settingsPath &&
    previous.status?.source === next.status?.source &&
    previous.status?.protocol === next.status?.protocol &&
    previous.status?.protocolVersion === next.status?.protocolVersion &&
    previous.status?.pid === next.status?.pid &&
    previous.status?.startedAt === next.status?.startedAt &&
    previous.status?.version === next.status?.version
  )
}

/** Polling owns only its own error; actions, preferences and full refresh failures survive it. */
function pollError(current: string | null, failed: boolean): string | null {
  if (current && current !== 'MCP_GATEWAY_POLL_FAILED') return current
  return failed ? 'MCP_GATEWAY_POLL_FAILED' : null
}

function emptySource(): McpGatewaySourceState {
  return { mode: 'grouped', view: null, connection: null, clients: [], busy: null, error: null }
}

export const useMcpGatewayStore = create<McpGatewayState>((set, get) => {
  const requests = { internal: 0, aggregator: 0 }
  const viewRequests = { internal: 0, aggregator: 0 }
  const modeRequests = { internal: 0, aggregator: 0 }
  let preferenceRequest = 0
  let modesRead = Promise.resolve(false)
  let preferenceWrite = Promise.resolve()
  const patch = (source: McpGatewaySource, data: Partial<McpGatewaySourceState>) =>
    set((state) => ({
      sources: { ...state.sources, [source]: { ...state.sources[source], ...data } }
    }))
  const applyLoadedMode = (source: McpGatewaySource, mode: McpGatewayMode) => {
    if (get().sources[source].mode === mode) return
    ++requests[source]
    patch(source, { mode, connection: null, clients: [] })
  }
  const saveModes = () => {
    // Serialize writes so a slow earlier preference cannot undo a later change.
    preferenceWrite = preferenceWrite
      .catch(() => {})
      .then(async () => {
        await modesRead
        const { internal, aggregator } = get().sources
        const result = await persistenceApi.write(MCP_GATEWAY_MODES_KEY, {
          internal: internal.mode,
          aggregator: aggregator.mode
        })
        if (!result.success) throw new Error('MCP_GATEWAY_PREFERENCE_FAILED')
      })
    return preferenceWrite
  }
  const act = async (
    source: McpGatewaySource,
    key: string,
    action: () => Promise<{ success: boolean }>
  ): Promise<boolean> => {
    if (get().sources[source].busy) return false
    patch(source, { busy: key, error: null })
    try {
      const result = await action()
      if (result.success) await get().refresh(source)
      else patch(source, { error: 'MCP_GATEWAY_ACTION_FAILED' })
      return result.success
    } catch {
      patch(source, { error: 'MCP_GATEWAY_ACTION_FAILED' })
      return false
    } finally {
      patch(source, { busy: null })
    }
  }

  return {
    sources: { internal: emptySource(), aggregator: emptySource() },
    loadMode: async () => {
      const sequence = ++preferenceRequest
      const selectedModes = { ...modeRequests }
      modesRead = (async () => {
        try {
          const stored = await persistenceApi.read<unknown>(MCP_GATEWAY_MODES_KEY)
          if (sequence !== preferenceRequest) return false
          if (stored.success && stored.data && typeof stored.data === 'object') {
            const modes = stored.data as Record<string, unknown>
            if (selectedModes.internal === modeRequests.internal && isMode(modes.internal))
              applyLoadedMode('internal', modes.internal)
            if (selectedModes.aggregator === modeRequests.aggregator && isMode(modes.aggregator))
              applyLoadedMode('aggregator', modes.aggregator)
            return false
          }
          const legacy = await persistenceApi.read<unknown>(MCP_GATEWAY_MODE_KEY)
          if (sequence !== preferenceRequest) return false
          if (legacy.success && isMode(legacy.data)) {
            if (selectedModes.internal === modeRequests.internal)
              applyLoadedMode('internal', legacy.data)
            return true
          }
        } catch {
          // Preferences are optional; a failed read must never disable a connection.
        }
        return false
      })()
      // Save migration only after the read resolves: queued user writes can safely await it too.
      if (await modesRead) {
        try {
          await saveModes()
        } catch {
          /* Keep the loaded mode usable if persistence fails. */
        }
      }
    },
    setMode: async (source, mode) => {
      const modeSequence = ++modeRequests[source]
      ++requests[source]
      patch(source, { mode, connection: null, clients: [], error: null })
      const refresh = get().refresh(source)
      try {
        await saveModes()
      } catch {
        if (modeSequence === modeRequests[source])
          patch(source, { error: 'MCP_GATEWAY_PREFERENCE_FAILED' })
      }
      await refresh
    },
    refresh: async (source) => {
      const sequence = ++requests[source]
      const viewSequence = ++viewRequests[source]
      const { mode } = get().sources[source]
      const [view, connection, clients] = await Promise.allSettled([
        mcpGatewayApi.view(source),
        mcpGatewayApi.connection(source, mode),
        mcpGatewayApi.clients(source, mode)
      ])
      if (sequence !== requests[source]) return
      const validView =
        view.status === 'fulfilled' &&
        view.value.success &&
        viewMatchesSource(view.value.data, source)
      const validConnection =
        connection.status === 'fulfilled' &&
        connection.value.success &&
        connection.value.data.source === source &&
        connection.value.data.mode === mode
      const validClients = clients.status === 'fulfilled' && clients.value.success
      const nextView =
        viewSequence === viewRequests[source]
          ? validView && view.status === 'fulfilled' && view.value.success
            ? view.value.data
            : null
          : get().sources[source].view
      const nextConnection =
        validConnection && connection.status === 'fulfilled' && connection.value.success
          ? connection.value.data
          : null
      patch(source, {
        ...(viewSequence === viewRequests[source] ? { view: nextView } : {}),
        connection:
          nextConnection && connectionMatchesView(nextConnection, nextView, source, mode)
            ? nextConnection
            : revokeHttp(nextConnection),
        clients:
          validClients && clients.status === 'fulfilled' && clients.value.success
            ? clients.value.data
            : [],
        error:
          (viewSequence === viewRequests[source] && !validView) || !validConnection || !validClients
            ? 'MCP_GATEWAY_REFRESH_FAILED'
            : get().sources[source].error === 'MCP_GATEWAY_REFRESH_FAILED'
              ? null
              : get().sources[source].error
      })
    },
    refreshView: async (source) => {
      const sequence = ++viewRequests[source]
      let connectionRequest: number | undefined
      const fail = () => {
        if (
          sequence !== viewRequests[source] ||
          (connectionRequest !== undefined && connectionRequest !== requests[source])
        )
          return
        ++requests[source] // A delayed full refresh or identity recheck cannot revive this bearer.
        const current = get().sources[source]
        patch(source, {
          view: null,
          connection: revokeHttp(current.connection),
          error: pollError(current.error, true)
        })
      }
      try {
        const result = await mcpGatewayApi.view(source)
        if (sequence !== viewRequests[source]) return
        if (!result.success || !viewMatchesSource(result.data, source)) {
          fail()
          return
        }
        const nextView = result.data
        const current = get().sources[source]
        if (nextView.state !== 'running') {
          ++requests[source]
          patch(source, {
            view: nextView,
            connection: revokeHttp(current.connection),
            error: pollError(current.error, nextView.state === 'error')
          })
          return
        }
        const needsConnection =
          !current.connection?.httpReady ||
          !connectionMatchesView(current.connection, nextView, source, current.mode) ||
          !sameRuntime(current.view, nextView)
        if (!needsConnection) {
          patch(source, { view: nextView, error: pollError(current.error, false) })
          return
        }
        const request = ++requests[source]
        connectionRequest = request
        const { mode } = current
        patch(source, {
          view: nextView,
          connection: revokeHttp(current.connection),
          error: pollError(current.error, false)
        })
        const resultConnection = await mcpGatewayApi.connection(source, mode)
        if (
          sequence !== viewRequests[source] ||
          request !== requests[source] ||
          mode !== get().sources[source].mode
        )
          return
        const latest = get().sources[source]
        if (
          !resultConnection.success ||
          !connectionMatchesView(resultConnection.data, latest.view, source, mode)
        ) {
          patch(source, {
            connection: revokeHttp(latest.connection),
            error: pollError(latest.error, true)
          })
          return
        }
        patch(source, {
          connection:
            resultConnection.data.httpReady === true
              ? resultConnection.data
              : revokeHttp(resultConnection.data),
          error: pollError(latest.error, false)
        })
      } catch {
        fail()
      }
    },
    start: (source) => act(source, 'start', () => mcpGatewayApi.start(source)),
    restart: (source) => act(source, 'restart', () => mcpGatewayApi.restart(source)),
    setPort: (source, port) => act(source, 'port', () => mcpGatewayApi.setPort(source, port)),
    syncClient: (source, id) =>
      act(source, `sync:${id}`, () =>
        mcpGatewayApi.syncClient(source, id, get().sources[source].mode)
      ),
    unsyncClient: (source, id) =>
      act(source, `sync:${id}`, () => mcpGatewayApi.unsyncClient(source, id))
  }
})
