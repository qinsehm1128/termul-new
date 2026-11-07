/**
 * Desktop facade for the standalone MCP gateway: its process, how agents
 * connect to it, and which local AI clients are already connected.
 *
 * The gateway is a desktop process, so every call reports
 * `DESKTOP_ONLY` in the browser build instead of throwing.
 */

import type { IpcResult } from '@shared/types/ipc.types'
import { MCP_GATEWAY_CLIENT_KEYS, type McpGatewaySource } from '@shared/types/mcp-gateway.types'
import { invoke } from '@tauri-apps/api/core'
import { logFrontendError } from '@/lib/log-api'
import { isTauriContext } from '@/lib/tauri-runtime'

/** Presentation mode within one isolated source; never selects the source. */
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
  source: McpGatewaySource
  protocol: string
  protocolVersion: number
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
  source: McpGatewaySource
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
  source: McpGatewaySource
  /** Current source identity and authentication have both been verified. */
  httpReady: boolean
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

export interface McpServerSummary {
  name: string
  /** The description agents read: a stored one, else what the server says. */
  description: string
  builtIn: boolean
}

export interface McpDescribed {
  name: string
  id: string
  description?: string
  error?: string
}

/** How the model writes descriptions. */
export interface McpDescribeSettings {
  /** System prompt; `{maxChars}` becomes `maxChars`. */
  prompt: string
  /** Longest description kept. */
  maxChars: number
  /** Each tool description in the prompt is cut to this; 0 keeps it whole. */
  toolDescriptionChars: number
}

/** Longest description stored, typed or generated. */
export const MCP_DESCRIPTION_MAX_CHARS = 2000

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
  const logFailure = () => {
    const source = args?.source === 'internal' ? 'internal' : 'aggregator'
    const mode = MCP_GATEWAY_MODES.includes(args?.mode as McpGatewayMode) ? args?.mode : 'none'
    void logFrontendError({
      source: 'mcp-gateway-api',
      message: `source=${source} mode=${mode} operation=${command} code=MCP_GATEWAY_REQUEST_FAILED`
    })
  }
  try {
    const result = await invoke<IpcResult<T>>(command, args)
    if (!result.success) logFailure()
    return result
  } catch {
    logFailure()
    return {
      success: false,
      error: 'MCP gateway request failed',
      code: 'INVOKE_ERROR'
    }
  }
}

export const mcpGatewayApi = {
  view: (source: McpGatewaySource) => call<McpGatewayView>('mcp_get_runtime_status', { source }),
  start: (source: McpGatewaySource) => call<McpGatewayView>('mcp_service_start', { source }),
  restart: (source: McpGatewaySource) => call<McpGatewayView>('mcp_service_restart', { source }),
  setPort: (source: McpGatewaySource, port: number) =>
    call<McpGatewayView>('mcp_service_set_port', { source, port }),
  connection: (source: McpGatewaySource, mode: McpGatewayMode) =>
    call<McpGatewayConnection>('mcp_connection_info', { source, mode }),
  clients: (source: McpGatewaySource, mode: McpGatewayMode) =>
    call<McpClient[]>('mcp_clients_detect', { source, mode }),
  syncClient: (source: McpGatewaySource, id: string, mode: McpGatewayMode) =>
    call<McpClientSyncOutcome>('mcp_client_sync', { source, id, mode }),
  unsyncClient: (source: McpGatewaySource, id: string) =>
    call<McpClientSyncOutcome>('mcp_client_unsync', { source, id }),
  servers: (source: McpGatewaySource) =>
    call<{ servers: McpServerSummary[] }>('mcp_service_servers', { source }),
  /** Stored descriptions, config id → text. */
  descriptions: () => call<Record<string, string>>('mcp_descriptions_get'),
  putDescriptions: (descriptions: Record<string, string>) =>
    call<void>('mcp_descriptions_put', { descriptions }),
  describeSettings: () =>
    call<{ settings: McpDescribeSettings; defaults: McpDescribeSettings }>(
      'mcp_describe_settings_get'
    ),
  putDescribeSettings: (settings: McpDescribeSettings) =>
    call<void>('mcp_describe_settings_put', { settings }),
  /** Have the configured model describe the named servers (all when omitted). */
  describe: (language: string, names?: string[]) =>
    call<McpDescribed[]>('mcp_describe_servers', { language, names: names ?? null })
}

/** The `mcpServers` JSON most clients (Claude Code, Cursor, …) accept. */
export function mcpServersJson(source: McpGatewaySource, bridge: McpBridgeCommand): string {
  return JSON.stringify(
    {
      mcpServers: {
        [MCP_GATEWAY_CLIENT_KEYS[source]]: { command: bridge.command, args: bridge.args }
      }
    },
    null,
    2
  )
}

function tomlString(value: string): string {
  return JSON.stringify(value)
}

/** The Codex `config.toml` table. */
export function codexToml(source: McpGatewaySource, bridge: McpBridgeCommand): string {
  const args = bridge.args.map(tomlString).join(', ')
  return [
    `[mcp_servers.${MCP_GATEWAY_CLIENT_KEYS[source]}]`,
    `command = ${tomlString(bridge.command)}`,
    `args = [${args}]`,
    'tool_timeout_sec = 300'
  ].join('\n')
}

/** Streamable HTTP entry for clients that prefer a URL over a command. */
export function httpServerJson(connection: McpGatewayConnection): string {
  if (connection.httpReady !== true) throw new Error('MCP_HTTP_NOT_READY')
  return JSON.stringify(
    {
      mcpServers: {
        [MCP_GATEWAY_CLIENT_KEYS[connection.source]]: {
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
