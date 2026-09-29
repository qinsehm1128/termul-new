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

export const MCP_GATEWAY_MODE_KEY = 'mcp/gateway-mode'

interface McpGatewayState {
  mode: McpGatewayMode
  view: McpGatewayView | null
  connection: McpGatewayConnection | null
  clients: McpClient[]
  /** Key of the action in flight (`start`, `port`, `sync:<id>`, …). */
  busy: string | null
  error: string | null
  loadMode: () => Promise<void>
  setMode: (mode: McpGatewayMode) => Promise<void>
  /** Gateway state, connection details and clients for the current mode. */
  refresh: () => Promise<void>
  /** Gateway state only; cheap enough to poll. */
  refreshView: () => Promise<void>
  start: () => Promise<boolean>
  restart: () => Promise<boolean>
  setPort: (port: number) => Promise<boolean>
  syncClient: (id: string) => Promise<boolean>
  unsyncClient: (id: string) => Promise<boolean>
}

function isMode(value: unknown): value is McpGatewayMode {
  return MCP_GATEWAY_MODES.includes(value as McpGatewayMode)
}

export const useMcpGatewayStore = create<McpGatewayState>((set, get) => {
  /** Run one action; its failure lands in `error`, success refreshes. */
  const act = async (
    key: string,
    action: () => Promise<{ success: boolean; error?: string }>
  ): Promise<boolean> => {
    set({ busy: key, error: null })
    const result = await action()
    set({ busy: null, error: result.success ? null : (result.error ?? key) })
    await get().refresh()
    return result.success
  }

  return {
    mode: 'grouped',
    view: null,
    connection: null,
    clients: [],
    busy: null,
    error: null,

    loadMode: async () => {
      const stored = await persistenceApi.read<unknown>(MCP_GATEWAY_MODE_KEY)
      if (stored.success && isMode(stored.data)) set({ mode: stored.data })
    },

    setMode: async (mode) => {
      set({ mode })
      await persistenceApi.write(MCP_GATEWAY_MODE_KEY, mode)
      await get().refresh()
    },

    refresh: async () => {
      const mode = get().mode
      const [view, connection, clients] = await Promise.all([
        mcpGatewayApi.view(),
        mcpGatewayApi.connection(mode),
        mcpGatewayApi.clients(mode)
      ])
      // A mode switch while this was loading owns the newer answer.
      if (get().mode !== mode) return
      set({
        view: view.success ? view.data : null,
        connection: connection.success ? connection.data : null,
        clients: clients.success ? clients.data : []
      })
    },

    refreshView: async () => {
      const view = await mcpGatewayApi.view()
      if (view.success) set({ view: view.data })
    },

    start: () => act('start', mcpGatewayApi.start),
    restart: () => act('restart', mcpGatewayApi.restart),
    setPort: (port) => act('port', () => mcpGatewayApi.setPort(port)),
    syncClient: (id) => act(`sync:${id}`, () => mcpGatewayApi.syncClient(id, get().mode)),
    unsyncClient: (id) => act(`sync:${id}`, () => mcpGatewayApi.unsyncClient(id))
  }
})
