/**
 * Unified MCP control-plane facade.
 *
 * Desktop uses Tauri commands (`mcp_get_config` / `mcp_put_config` /
 * `mcp_get_status`). Browser/shared-live uses the equivalent HTTP routes
 * (`GET/PUT /mcp/config`, `GET /mcp/status`). Probe stays on the existing
 * canonical `acp-mcp-probe` facade (`acp_probe_mcp_server` /
 * `POST /mcp/probe` alias of `/mcp-servers/probe`).
 *
 * Renderer business code must import this module rather than `@tauri-apps/*`.
 */

import type { IpcResult } from '@shared/types/ipc.types'
import { invoke } from '@tauri-apps/api/core'
import {
  loadMcpServers as loadLegacyMcpServers,
  normalizeMcpRegistry,
  type StoredMcpServer
} from '@/lib/acp-mcp-persistence'
import { isTauriContext } from '@/lib/tauri-runtime'
import { webServerMcpControl } from '@/lib/web-server-api'

export const MCP_CONTROL_PLANE_SCHEMA_VERSION = 1
export const BUILTIN_SESSION_MEMORY = 'session-memory'
export const BUILTIN_PROJECT_SCOPE = 'project-scope'

export type McpNameCollisionPolicy = 'prefixServerId'

export interface McpCapabilityPolicy {
  allowTools?: string[]
  denyTools?: string[]
}

export interface McpBuiltInConfig {
  id: string
  enabled: boolean
  policy?: McpCapabilityPolicy
}

export interface McpSecretPair {
  name: string
  value?: string
  ref?: string
}

export type McpUpstreamConfig = StoredMcpServer & {
  policy?: McpCapabilityPolicy
}

export interface McpRoutingConfig {
  nameCollision: McpNameCollisionPolicy
}

export interface McpControlPlaneConfig {
  schemaVersion: number
  revision: number
  builtIns: McpBuiltInConfig[]
  upstreams: McpUpstreamConfig[]
  routing: McpRoutingConfig
}

export interface McpBuiltInStatus {
  id: string
  enabled: boolean
}

export interface McpUpstreamStatus {
  id: string
  name: string
  enabled: boolean
  type: 'stdio' | 'http' | 'sse' | string
}

export interface McpControlPlaneStatus {
  schemaVersion: number
  revision: number
  builtIns: McpBuiltInStatus[]
  upstreams: McpUpstreamStatus[]
  routing: McpRoutingConfig
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function defaultBuiltIns(): McpBuiltInConfig[] {
  return [
    { id: BUILTIN_SESSION_MEMORY, enabled: true },
    { id: BUILTIN_PROJECT_SCOPE, enabled: true }
  ]
}

export function emptyMcpConfig(): McpControlPlaneConfig {
  return {
    schemaVersion: MCP_CONTROL_PLANE_SCHEMA_VERSION,
    revision: 0,
    builtIns: defaultBuiltIns(),
    upstreams: [],
    routing: { nameCollision: 'prefixServerId' }
  }
}

function fillDefaultBuiltIns(builtIns: McpBuiltInConfig[]): McpBuiltInConfig[] {
  const next = [...builtIns]
  for (const fallback of defaultBuiltIns()) {
    if (!next.some((item) => item.id === fallback.id)) next.push(fallback)
  }
  return next
}

function normalizeBuiltIn(value: unknown): McpBuiltInConfig | null {
  if (!isRecord(value) || typeof value.id !== 'string' || value.id.trim().length === 0) return null
  if (value.enabled !== undefined && typeof value.enabled !== 'boolean') return null
  return { id: value.id, enabled: value.enabled ?? true }
}

export function normalizeMcpConfig(value: unknown): McpControlPlaneConfig {
  if (Array.isArray(value)) {
    return {
      ...emptyMcpConfig(),
      upstreams: normalizeMcpRegistry(value)
    }
  }
  if (!isRecord(value)) return emptyMcpConfig()
  const schemaVersion =
    typeof value.schemaVersion === 'number' ? value.schemaVersion : MCP_CONTROL_PLANE_SCHEMA_VERSION
  const revision = typeof value.revision === 'number' ? value.revision : 0
  const builtIns = Array.isArray(value.builtIns)
    ? value.builtIns.flatMap((entry) => {
        const item = normalizeBuiltIn(entry)
        return item ? [item] : []
      })
    : []
  const routing =
    isRecord(value.routing) && value.routing.nameCollision === 'prefixServerId'
      ? { nameCollision: 'prefixServerId' as const }
      : { nameCollision: 'prefixServerId' as const }
  const upstreams = Array.isArray(value.upstreams)
    ? normalizeMcpRegistry(value.upstreams)
    : normalizeMcpRegistry(value)
  return {
    schemaVersion,
    revision,
    builtIns: fillDefaultBuiltIns(builtIns),
    upstreams,
    routing
  }
}

function normalizeStatus(value: unknown): McpControlPlaneStatus {
  if (!isRecord(value)) {
    const empty = emptyMcpConfig()
    return {
      schemaVersion: empty.schemaVersion,
      revision: empty.revision,
      builtIns: empty.builtIns.map(({ id, enabled }) => ({ id, enabled })),
      upstreams: [],
      routing: empty.routing
    }
  }
  const config = normalizeMcpConfig(value)
  return {
    schemaVersion: config.schemaVersion,
    revision: config.revision,
    builtIns: Array.isArray(value.builtIns)
      ? value.builtIns.flatMap((entry) => {
          const item = normalizeBuiltIn(entry)
          return item ? [{ id: item.id, enabled: item.enabled }] : []
        })
      : config.builtIns.map(({ id, enabled }) => ({ id, enabled })),
    upstreams: Array.isArray(value.upstreams)
      ? value.upstreams.flatMap((entry) => {
          if (!isRecord(entry) || typeof entry.id !== 'string' || typeof entry.name !== 'string') {
            return []
          }
          return [
            {
              id: entry.id,
              name: entry.name,
              enabled: entry.enabled !== false,
              type: typeof entry.type === 'string' ? entry.type : 'stdio'
            }
          ]
        })
      : config.upstreams.map((upstream) => ({
          id: upstream.id,
          name: upstream.name,
          enabled: upstream.enabled !== false,
          type: upstream.type ?? 'stdio'
        })),
    routing: config.routing
  }
}

async function invokeIpc<T>(
  command: string,
  args?: Record<string, unknown>
): Promise<IpcResult<T>> {
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

async function migrateLegacyDesktopConfig(): Promise<McpControlPlaneConfig> {
  const legacy = await loadLegacyMcpServers()
  if (legacy.length === 0) return emptyMcpConfig()
  const pending: McpControlPlaneConfig = {
    ...emptyMcpConfig(),
    revision: 1,
    upstreams: legacy
  }
  const written = await putMcpConfig(pending)
  if (written.success) return written.data
  return pending
}

export async function getMcpConfig(): Promise<IpcResult<McpControlPlaneConfig>> {
  if (isTauriContext()) {
    const res = await invokeIpc<unknown>('mcp_get_config')
    if (res.success) return { success: true, data: normalizeMcpConfig(res.data) }
    if (res.code === 'MCP_REGISTRY_NOT_FOUND' || res.code === 'NO_ACTIVE_PROJECT_ROOT') {
      try {
        return { success: true, data: await migrateLegacyDesktopConfig() }
      } catch (error) {
        return {
          success: false,
          error: error instanceof Error ? error.message : String(error),
          code: 'MCP_REGISTRY_READ_ERROR'
        }
      }
    }
    return { success: false, error: res.error, code: res.code }
  }
  const res = await webServerMcpControl.getConfig()
  if (!res.success) return res
  return { success: true, data: normalizeMcpConfig(res.data) }
}

export async function putMcpConfig(
  config: McpControlPlaneConfig
): Promise<IpcResult<McpControlPlaneConfig>> {
  const payload: McpControlPlaneConfig = {
    ...config,
    schemaVersion: MCP_CONTROL_PLANE_SCHEMA_VERSION,
    builtIns: fillDefaultBuiltIns(config.builtIns),
    routing: config.routing ?? { nameCollision: 'prefixServerId' }
  }
  if (isTauriContext()) {
    const res = await invokeIpc<unknown>('mcp_put_config', { config: payload })
    if (!res.success) return { success: false, error: res.error, code: res.code }
    return { success: true, data: normalizeMcpConfig(res.data) }
  }
  const res = await webServerMcpControl.putConfig(payload)
  if (!res.success) return res
  return { success: true, data: normalizeMcpConfig(res.data) }
}

export async function getMcpStatus(): Promise<IpcResult<McpControlPlaneStatus>> {
  if (isTauriContext()) {
    const res = await invokeIpc<unknown>('mcp_get_status')
    if (!res.success) {
      if (res.code === 'MCP_REGISTRY_NOT_FOUND' || res.code === 'NO_ACTIVE_PROJECT_ROOT') {
        return { success: true, data: normalizeStatus(emptyMcpConfig()) }
      }
      return { success: false, error: res.error, code: res.code }
    }
    return { success: true, data: normalizeStatus(res.data) }
  }
  const res = await webServerMcpControl.getStatus()
  if (!res.success) return res
  return { success: true, data: normalizeStatus(res.data) }
}
