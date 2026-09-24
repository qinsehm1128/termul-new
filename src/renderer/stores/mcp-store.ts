import { create } from 'zustand'
import { runtimeT } from '@/i18n/runtime'
import type { McpServerConfig, McpToolInfo, ProbeResult, ProbeStatus } from '@/lib/acp-api'
import { type StoredMcpServer, transportOf } from '@/lib/acp-mcp-persistence'
import { probeMcpServer as probeMcpServerConfig } from '@/lib/acp-mcp-probe'
import { logFrontendError } from '@/lib/log-api'
import {
  emptyMcpConfig,
  getMcpConfig,
  getMcpStatus,
  type McpControlPlaneConfig,
  type McpControlPlaneStatus,
  putMcpConfig
} from '@/lib/mcp-api'

export class McpRevisionConflictError extends Error {
  readonly code = 'MCP_REGISTRY_REVISION_CONFLICT'
  constructor(message: string) {
    super(message)
    this.name = 'McpRevisionConflictError'
  }
}

interface McpState {
  config: McpControlPlaneConfig
  loaded: boolean
  loadError: string | null
  saveError: string | null
  status: McpControlPlaneStatus | null
  probeStatus: Record<string, ProbeStatus>
  probeError: Record<string, string | undefined>
  tools: Record<string, McpToolInfo[]>
  toolsLoaded: Record<string, boolean>
  probing: Record<string, boolean>
  load: () => Promise<void>
  loadStatus: () => Promise<void>
  saveConfig: (next: McpControlPlaneConfig) => Promise<McpControlPlaneConfig>
  saveUpstream: (server: StoredMcpServer) => Promise<void>
  importUpstreams: (servers: StoredMcpServer[]) => Promise<void>
  setUpstreamEnabled: (id: string, enabled: boolean) => Promise<void>
  deleteUpstream: (id: string) => Promise<void>
  setBuiltInEnabled: (id: string, enabled: boolean) => Promise<void>
  probe: (id: string) => Promise<void>
  loadTools: (id: string) => Promise<void>
}

let mcpMutationQueue: Promise<unknown> = Promise.resolve()
async function runSerialized(mutation: () => Promise<void>): Promise<void> {
  const run = mcpMutationQueue.then(mutation)
  mcpMutationQueue = run.catch(() => undefined)
  await run
}

function toWireConfig(server: StoredMcpServer): McpServerConfig {
  const { id: _id, enabled: _enabled, ...config } = server
  return config as McpServerConfig
}

export const useMcpStore = create<McpState>((set, get) => ({
  config: emptyMcpConfig(),
  loaded: false,
  loadError: null,
  saveError: null,
  status: null,
  probeStatus: {},
  probeError: {},
  tools: {},
  toolsLoaded: {},
  probing: {},

  load: async () => {
    try {
      const res = await getMcpConfig()
      if (!res.success) {
        throw new Error(
          res.error ?? runtimeT('mcp', 'persistence.loadFailed', 'Failed to load MCP servers')
        )
      }
      set({ config: res.data, loaded: true, loadError: null })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      set({ loaded: true, loadError: message })
      void logFrontendError({
        source: 'mcp-store.load',
        message: `Failed to load MCP control-plane config (${message})`
      })
    }
  },

  loadStatus: async () => {
    const res = await getMcpStatus()
    if (!res.success) {
      void logFrontendError({
        source: 'mcp-store.loadStatus',
        message: `Failed to load MCP status (${res.error ?? res.code ?? 'unknown'})`
      })
      return
    }
    set({ status: res.data })
  },

  saveConfig: async (next) => {
    const previous = get().config
    set({ config: next, saveError: null })
    const res = await putMcpConfig(next)
    if (!res.success) {
      set({ config: previous, saveError: res.error ?? res.code ?? 'save failed' })
      if (res.code === 'MCP_REGISTRY_REVISION_CONFLICT') {
        await get().load()
        throw new McpRevisionConflictError(
          res.error ?? runtimeT('mcp', 'page.revisionConflict', 'MCP config was updated elsewhere.')
        )
      }
      throw new Error(
        res.error ?? runtimeT('mcp', 'persistence.saveFailed', 'Failed to persist MCP servers')
      )
    }
    set({ config: res.data, loaded: true, saveError: null })
    return res.data
  },

  saveUpstream: (server) =>
    runSerialized(async () => {
      const list = get().config.upstreams
      const idx = list.findIndex((item) => item.id === server.id)
      const nextServer = { ...server, enabled: server.enabled ?? true }
      const nextUpstreams =
        idx === -1
          ? [...list, nextServer]
          : list.map((item) => (item.id === server.id ? nextServer : item))
      await get().saveConfig({
        ...get().config,
        revision: get().config.revision + 1,
        upstreams: nextUpstreams
      })
    }),

  importUpstreams: async (servers) => {
    if (servers.length === 0) return
    await runSerialized(async () => {
      await get().saveConfig({
        ...get().config,
        revision: get().config.revision + 1,
        upstreams: [...get().config.upstreams, ...servers]
      })
    })
  },

  setUpstreamEnabled: (id, enabled) =>
    runSerialized(async () => {
      await get().saveConfig({
        ...get().config,
        revision: get().config.revision + 1,
        upstreams: get().config.upstreams.map((server) =>
          server.id === id ? { ...server, enabled } : server
        )
      })
    }),

  deleteUpstream: (id) =>
    runSerialized(async () => {
      await get().saveConfig({
        ...get().config,
        revision: get().config.revision + 1,
        upstreams: get().config.upstreams.filter((server) => server.id !== id)
      })
    }),

  setBuiltInEnabled: (id, enabled) =>
    runSerialized(async () => {
      await get().saveConfig({
        ...get().config,
        revision: get().config.revision + 1,
        builtIns: get().config.builtIns.map((item) =>
          item.id === id ? { ...item, enabled } : item
        )
      })
    }),

  probe: async (id) => {
    if (get().probing[id]) return
    const server = get().config.upstreams.find((item) => item.id === id)
    if (!server) return
    set((state) => ({ probing: { ...state.probing, [id]: true } }))
    try {
      const result: ProbeResult = await probeMcpServerConfig(toWireConfig(server))
      set((state) => ({
        probeStatus: { ...state.probeStatus, [id]: result.status },
        tools: { ...state.tools, [id]: result.tools },
        toolsLoaded: { ...state.toolsLoaded, [id]: true },
        probing: { ...state.probing, [id]: false },
        probeError: {
          ...state.probeError,
          [id]: result.status === 'connected' ? undefined : result.error
        }
      }))
    } catch (error) {
      set((state) => ({
        probeStatus: { ...state.probeStatus, [id]: 'disconnected' },
        tools: { ...state.tools, [id]: [] },
        toolsLoaded: { ...state.toolsLoaded, [id]: false },
        probing: { ...state.probing, [id]: false },
        probeError: { ...state.probeError, [id]: undefined }
      }))
      void logFrontendError({
        source: 'mcp-store.probe',
        message: `MCP probe failed for server '${server.name}' (${String(error)})`
      })
    }
  },

  loadTools: async (id) => {
    if (get().toolsLoaded[id] || get().probing[id]) return
    await get().probe(id)
  }
}))

export function mcpUpstreamDetail(server: StoredMcpServer): string {
  return transportOf(server) === 'stdio'
    ? (server as Extract<StoredMcpServer, { type?: 'stdio' }>).command
    : (server as Extract<StoredMcpServer, { type: 'http' | 'sse' }>).url
}
