import type { McpServerConfig } from '@/lib/acp-api'
import type { StoredMcpServer } from '@/lib/acp-mcp-persistence'
import type { McpControlPlaneConfig, McpUpstreamConfig } from '@/lib/mcp-api'
import { randomUUID } from '@/lib/uuid'
import { parseMcpJsonImport } from './mcp-json-import'

/**
 * The MCP export is a canonical control-plane document with one additive
 * compatibility field. `schemaVersion`, `revision`, `builtIns`, `upstreams`,
 * and `routing` remain unchanged so the file can also be used as a canonical
 * registry payload. `mcpServers` is the ordinary Claude/VS Code wrapper and is
 * intentionally derived from `upstreams` rather than treated as a second
 * persistence authority.
 */
export interface McpJsonExport extends McpControlPlaneConfig {
  mcpServers: Record<string, Record<string, unknown>>
}

export interface PreparedMcpJsonImport {
  servers: StoredMcpServer[]
  errors: string[]
  skipped: number
}

function compatibilityPairs(
  pairs: Array<{ name: string; value: string }> | undefined
): Record<string, string> | undefined {
  if (!pairs || pairs.length === 0) return undefined
  const result: Record<string, string> = {}
  for (const pair of pairs) result[pair.name] = pair.value
  return result
}

function compatibilityServer(server: McpUpstreamConfig): Record<string, unknown> {
  const type = server.type ?? 'stdio'
  if (type === 'stdio') {
    const stdio = server as Extract<McpServerConfig, { type?: 'stdio' }>
    const env = compatibilityPairs(stdio.env)
    return {
      command: stdio.command,
      ...(stdio.args && stdio.args.length > 0 ? { args: stdio.args } : {}),
      ...(env ? { env } : {})
    }
  }

  const remote = server as Extract<McpServerConfig, { type: 'http' | 'sse' }>
  const headers = compatibilityPairs(remote.headers)
  return {
    ...(type === 'sse' ? { type: 'sse' } : {}),
    url: remote.url,
    ...(headers ? { headers } : {})
  }
}

function uniqueCompatibilityName(
  requestedName: string,
  server: McpUpstreamConfig,
  map: Record<string, Record<string, unknown>>
): string {
  const base = requestedName.trim() || server.id || 'mcp-server'
  if (!Object.prototype.hasOwnProperty.call(map, base)) return base
  let suffix = 2
  while (Object.prototype.hasOwnProperty.call(map, `${base} (${suffix})`)) suffix += 1
  return `${base} (${suffix})`
}

/** Build the canonical-plus-compatibility object without serializing it. */
export function buildMcpJsonExport(config: McpControlPlaneConfig): McpJsonExport {
  const mcpServers: Record<string, Record<string, unknown>> = {}
  for (const server of config.upstreams) {
    const key = uniqueCompatibilityName(server.name, server, mcpServers)
    mcpServers[key] = compatibilityServer(server)
  }
  return { ...config, mcpServers }
}

/** Stable, human-readable JSON suitable for a browser/Tauri file download. */
export function serializeMcpJsonExport(config: McpControlPlaneConfig): string {
  return JSON.stringify(buildMcpJsonExport(config), null, 2)
}

/** Download through ordinary browser APIs; works in both shared-live and Tauri webviews. */
export function downloadMcpJsonExport(
  config: McpControlPlaneConfig,
  filename = 'mcp-servers.json'
): void {
  const url = URL.createObjectURL(
    new Blob([serializeMcpJsonExport(config)], { type: 'application/json' })
  )
  try {
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = filename
    anchor.click()
  } finally {
    URL.revokeObjectURL(url)
  }
}

// Descriptive aliases keep the helper convenient for callers that use the
// control-plane terminology rather than the wire-file terminology.
export const buildMcpControlPlaneExport = buildMcpJsonExport
export const serializeMcpControlPlaneExport = serializeMcpJsonExport

/**
 * Parse an import file, remove name collisions with the current registry, and
 * assign fresh ids. Built-ins and all existing upstreams are left to the store;
 * this helper returns only the append batch for `importUpstreams`.
 */
export function prepareMcpJsonImport(
  text: string,
  current: Array<Pick<StoredMcpServer, 'name'> & { id?: string }>,
  makeId: () => string = randomUUID
): PreparedMcpJsonImport {
  const parsed = parseMcpJsonImport(text)
  const seenNames = new Set(current.map((server) => server.name.trim().toLocaleLowerCase()))
  const usedIds = new Set(current.flatMap((server) => (server.id ? [server.id] : [])))
  const servers: StoredMcpServer[] = []
  let skipped = 0

  for (const server of parsed.servers) {
    const key = server.name.trim().toLocaleLowerCase()
    if (seenNames.has(key)) {
      skipped += 1
      continue
    }
    seenNames.add(key)
    let id = makeId()
    while (usedIds.has(id)) id = makeId()
    usedIds.add(id)
    servers.push({ ...server, id, enabled: server.enabled ?? true } as StoredMcpServer)
  }

  return { servers, errors: parsed.errors, skipped }
}
