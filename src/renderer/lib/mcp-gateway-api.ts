/**
 * Desktop facade for the standalone MCP gateway: its process, how agents
 * connect to it, and which local AI clients are already connected.
 *
 * The gateway is a desktop process, so every call reports
 * `DESKTOP_ONLY` in the browser build instead of throwing.
 */

import type { IpcResult } from '@shared/types/ipc.types'
import { invoke } from '@tauri-apps/api/core'
import { isTauriContext } from '@/lib/tauri-runtime'

/** How the gateway exposes the aggregated servers to an agent. */
export type McpGatewayMode = 'grouped' | 'entry' | 'direct'

export const MCP_GATEWAY_MODES: readonly McpGatewayMode[] = ['grouped', 'entry', 'direct']

export type McpUpstreamState = 'disabled' | 'connecting' | 'connected' | 'failed'

export interface McpGatewayUpstream {
  id: string
  /** Route name agents use, e.g. `context7_tool_list`. */
  name: string
  state: McpUpstreamState
  error?: string
}

export interface McpGatewayStatus {
  version: string
  pid: number
  port: number
  startedAt: number
  configRevision: number | null
  configError: string | null
  upstreams: McpGatewayUpstream[]
  builtIns: string[]
}

export interface McpGatewayView {
  state: 'running' | 'stopped' | 'error'
  port: number | null
  settingsPath: string
  error?: string
  status?: McpGatewayStatus
}

export interface McpBridgeCommand {
  command: string
  args: string[]
}

export interface McpGatewayConnection {
  mode: McpGatewayMode
  bridge: McpBridgeCommand
  /** `se-mcp` exists next to the app binary. */
  bridgeAvailable: boolean
  url: string
  /** Bearer token for the HTTP endpoint. Never log it. */
  token: string
  port: number
  settingsPath: string
}

export interface McpClient {
  id: string
  name: string
  configPath: string
  installed: boolean
  synced: boolean
  upToDate: boolean
  error?: string
}

export interface McpClientSyncOutcome {
  configPath: string
  backupPath?: string
}

const DESKTOP_ONLY: IpcResult<never> = {
  success: false,
  error: 'The MCP gateway is managed by the desktop app',
  code: 'DESKTOP_ONLY'
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<IpcResult<T>> {
  if (!isTauriContext()) return DESKTOP_ONLY
  try {
    return await invoke<IpcResult<T>>(command, args)
  } catch (error) {
    return {
      success: false,
      error: error instanceof Error ? error.message : String(error),
      code: 'INVOKE_ERROR'
    }
  }
}

export const mcpGatewayApi = {
  view: () => call<McpGatewayView>('mcp_get_runtime_status'),
  start: () => call<McpGatewayView>('mcp_service_start'),
  restart: () => call<McpGatewayView>('mcp_service_restart'),
  setPort: (port: number) => call<McpGatewayView>('mcp_service_set_port', { port }),
  connection: (mode: McpGatewayMode) => call<McpGatewayConnection>('mcp_connection_info', { mode }),
  clients: (mode: McpGatewayMode) => call<McpClient[]>('mcp_clients_detect', { mode }),
  syncClient: (id: string, mode: McpGatewayMode) =>
    call<McpClientSyncOutcome>('mcp_client_sync', { id, mode }),
  unsyncClient: (id: string) => call<McpClientSyncOutcome>('mcp_client_unsync', { id })
}

/** The `mcpServers` JSON most clients (Claude Code, Cursor, …) accept. */
export function mcpServersJson(bridge: McpBridgeCommand): string {
  return JSON.stringify(
    { mcpServers: { 'se-mcp': { command: bridge.command, args: bridge.args } } },
    null,
    2
  )
}

function tomlString(value: string): string {
  return JSON.stringify(value)
}

/** The Codex `config.toml` table. */
export function codexToml(bridge: McpBridgeCommand): string {
  const args = bridge.args.map(tomlString).join(', ')
  return [
    '[mcp_servers.se-mcp]',
    `command = ${tomlString(bridge.command)}`,
    `args = [${args}]`,
    'tool_timeout_sec = 300'
  ].join('\n')
}

/** Streamable HTTP entry for clients that prefer a URL over a command. */
export function httpServerJson(connection: McpGatewayConnection): string {
  return JSON.stringify(
    {
      mcpServers: {
        'se-mcp': {
          type: 'http',
          url: connection.url,
          headers: { Authorization: `Bearer ${connection.token}` }
        }
      }
    },
    null,
    2
  )
}
