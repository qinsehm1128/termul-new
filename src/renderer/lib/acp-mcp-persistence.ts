import { runtimeT } from '@/i18n/runtime'
import type { AgentCapabilities, McpServer, McpServerConfig } from '@/lib/acp-api'
import { persistenceApi } from '@/lib/api'
import { loadMcpRegistryFromProject, syncMcpRegistryToProject } from '@/lib/tauri-remote-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { webServerMcpServers } from '@/lib/web-server-api'
import { logFrontendError } from './log-api'

export const ACP_MCP_KEY = 'acp/mcp-servers'

export type McpTransport = 'stdio' | 'http' | 'sse'

/** Credential-free OAuth metadata. Tokens stay in the host keyring. */
export type McpAuthMode = 'none' | 'static' | 'oauth'

export type McpOAuthRegistrationMode = 'none' | 'preregistered' | 'dynamic' | 'clientMetadata'

export interface McpOAuthEndpoints {
  protectedResourceMetadataUrl?: string
  authorizationServerMetadataUrl?: string
  issuer?: string
  authorizationEndpoint?: string
  tokenEndpoint?: string
  registrationEndpoint?: string
  revocationEndpoint?: string
}

/** Unix `discoveredAt` is seconds. Secret fields are not part of this type. */
export interface McpOAuthConfig {
  authMode: McpAuthMode
  registrationMode: McpOAuthRegistrationMode
  clientId?: string
  clientMetadataUrl?: string
  scopes?: string[]
  endpoints?: McpOAuthEndpoints
  discoveredAt?: number
}

export type StoredMcpServer = McpServerConfig & {
  id: string
  enabled?: boolean
  oauth?: McpOAuthConfig
}

export interface McpValidation {
  valid: boolean
  errors: string[]
}

export interface SkippedMcpServer {
  id: string
  name: string
  transport: 'http' | 'sse'
}

export interface McpServerSelection {
  servers: McpServer[]
  skipped: SkippedMcpServer[]
  pending: boolean
}

export function transportOf(server: McpServerConfig): McpTransport {
  return (server.type ?? 'stdio') as McpTransport
}

export function validateMcpServer(server: Partial<McpServerConfig>): McpValidation {
  const errors: string[] = []
  if (!server.name || server.name.trim().length === 0) {
    errors.push(runtimeT('mcp', 'validation.nameRequired', 'Name is required.'))
  }
  const type = (server.type ?? 'stdio') as McpTransport
  if (type === 'stdio') {
    const value = server as Partial<{ command: string }>
    if (!value.command || value.command.trim().length === 0) {
      errors.push(
        runtimeT('mcp', 'validation.commandRequiredForStdio', 'Command is required for stdio.')
      )
    }
  } else {
    const value = server as Partial<{ url: string }>
    if (!value.url || value.url.trim().length === 0) {
      errors.push(runtimeT('mcp', 'validation.urlRequired', 'URL is required.'))
    } else {
      try {
        new URL(value.url)
      } catch {
        errors.push(runtimeT('mcp', 'validation.urlInvalid', 'URL is invalid.'))
      }
    }
  }
  return { valid: errors.length === 0, errors }
}

function toWireServer(entry: StoredMcpServer): McpServer {
  const { id: _id, enabled: _enabled, ...server } = entry
  // The ACP `McpServer` schema requires `args` + `env` (stdio) and `headers`
  // (http/sse) as non-optional arrays. The on-disk normalizer omits these
  // when empty, so re-fill them here to keep the wire payload deserializable.
  switch (transportOf(server)) {
    case 'stdio': {
      const { name, command, args, env } = server as Extract<McpServerConfig, { type?: 'stdio' }>
      return { type: 'stdio', name, command, args: args ?? [], env: env ?? [] }
    }
    case 'http': {
      const { name, url, headers } = server as Extract<McpServerConfig, { type: 'http' }>
      return { type: 'http', name, url, headers: headers ?? [] }
    }
    case 'sse': {
      const { name, url, headers } = server as Extract<McpServerConfig, { type: 'sse' }>
      return { type: 'sse', name, url, headers: headers ?? [] }
    }
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function stringPairs(value: unknown): Array<{ name: string; value: string }> | undefined {
  if (value === undefined) return undefined
  if (Array.isArray(value)) {
    const pairs = value.filter(
      (entry): entry is { name: string; value: string } =>
        isRecord(entry) && typeof entry.name === 'string' && typeof entry.value === 'string'
    )
    return pairs.length === value.length ? pairs : undefined
  }
  if (isRecord(value)) {
    const entries = Object.entries(value)
    return entries.every(([, entry]) => typeof entry === 'string')
      ? entries.map(([name, entry]) => ({ name, value: entry as string }))
      : undefined
  }
  return undefined
}

const OAUTH_SECRET_KEYS = new Set([
  'accessToken',
  'access_token',
  'refreshToken',
  'refresh_token',
  'clientSecret',
  'client_secret',
  'idToken',
  'id_token',
  'token',
  'bearerToken',
  'bearer_token'
])

function readOptionalString(
  record: Record<string, unknown>,
  keys: string[]
): string | undefined | null {
  for (const key of keys) {
    if (!Object.prototype.hasOwnProperty.call(record, key)) continue
    if (OAUTH_SECRET_KEYS.has(key)) continue
    const value = record[key]
    if (typeof value !== 'string') return null
    return value
  }
  return undefined
}

function hasControlCharacter(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index)
    if (code <= 31 || code === 127) return true
  }
  return false
}

function isPublicHttpUrl(value: string): boolean {
  try {
    const url = new URL(value)
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return false
    if (url.username || url.password) return false
    return url.hostname.length > 0
  } catch {
    return false
  }
}

function normalizeOptionalUrl(value: string | undefined): string | undefined | null {
  if (value === undefined) return undefined
  const trimmed = value.trim()
  if (trimmed.length === 0) return undefined
  return isPublicHttpUrl(trimmed) ? trimmed : null
}

function normalizeScopes(value: unknown): string[] | undefined | null {
  if (value === undefined) return undefined
  let parts: unknown[]
  if (typeof value === 'string') {
    parts = value.trim().split(/\s+/).filter(Boolean)
    if (parts.length === 0) return undefined
  } else if (Array.isArray(value)) {
    parts = value
  } else {
    return null
  }
  const scopes: string[] = []
  for (const entry of parts) {
    if (typeof entry !== 'string') return null
    const trimmed = entry.trim()
    if (trimmed.length === 0 || hasControlCharacter(trimmed) || /\s/.test(trimmed)) {
      return null
    }
    if (!scopes.includes(trimmed)) scopes.push(trimmed)
  }
  return scopes.length > 0 ? scopes : undefined
}

function normalizeEndpoints(value: unknown): McpOAuthEndpoints | undefined | null {
  if (value === undefined || value === null) return undefined
  if (!isRecord(value)) return null
  const fields: Array<[keyof McpOAuthEndpoints, string[]]> = [
    [
      'protectedResourceMetadataUrl',
      ['protectedResourceMetadataUrl', 'protected_resource_metadata_url']
    ],
    [
      'authorizationServerMetadataUrl',
      ['authorizationServerMetadataUrl', 'authorization_server_metadata_url']
    ],
    ['issuer', ['issuer']],
    ['authorizationEndpoint', ['authorizationEndpoint', 'authorization_endpoint']],
    ['tokenEndpoint', ['tokenEndpoint', 'token_endpoint']],
    ['registrationEndpoint', ['registrationEndpoint', 'registration_endpoint']],
    ['revocationEndpoint', ['revocationEndpoint', 'revocation_endpoint']]
  ]
  const endpoints: McpOAuthEndpoints = {}
  for (const [canonical, keys] of fields) {
    const raw = readOptionalString(value, keys)
    if (raw === null) return null
    const url = normalizeOptionalUrl(raw)
    if (url === null) return null
    if (url) endpoints[canonical] = url
  }
  return Object.keys(endpoints).length > 0 ? endpoints : undefined
}

/**
 * Keep credential-free OAuth metadata.
 * `undefined` means absent or blank, `null` means the object is malformed.
 * Access tokens, refresh tokens, and client secrets are ignored.
 */
export function normalizeOAuthConfig(value: unknown): McpOAuthConfig | undefined | null {
  if (value === undefined || value === null) return undefined
  if (!isRecord(value)) return null
  const authMode = readOptionalString(value, ['authMode', 'auth_mode'])
  if (authMode === null) return null
  const registrationMode = readOptionalString(value, ['registrationMode', 'registration_mode'])
  if (registrationMode === null) return null
  const allowedAuth: readonly McpAuthMode[] = ['none', 'static', 'oauth']
  const allowedRegistration: readonly McpOAuthRegistrationMode[] = [
    'none',
    'preregistered',
    'dynamic',
    'clientMetadata'
  ]
  const normalizedAuth = (authMode ?? 'none') as McpAuthMode
  const normalizedRegistration = (registrationMode ?? 'none') as McpOAuthRegistrationMode
  if (authMode !== undefined && !allowedAuth.includes(normalizedAuth)) return null
  if (registrationMode !== undefined && !allowedRegistration.includes(normalizedRegistration)) {
    return null
  }
  const clientIdRaw = readOptionalString(value, ['clientId', 'client_id'])
  if (clientIdRaw === null) return null
  const clientId = clientIdRaw?.trim()
  if (
    clientId &&
    (clientId.length > 2048 || hasControlCharacter(clientId) || /\s/.test(clientId))
  ) {
    return null
  }
  const metadataRaw = readOptionalString(value, ['clientMetadataUrl', 'client_metadata_url'])
  if (metadataRaw === null) return null
  const clientMetadataUrl = normalizeOptionalUrl(metadataRaw)
  if (clientMetadataUrl === null) return null
  const scopes = normalizeScopes(value.scopes)
  if (scopes === null) return null
  const endpoints = normalizeEndpoints(value.endpoints)
  if (endpoints === null) return null
  const discoveredRaw = readTimestamp(value, ['discoveredAt', 'discovered_at'])
  if (discoveredRaw === null) return null
  const config: McpOAuthConfig = {
    authMode: normalizedAuth,
    registrationMode: normalizedRegistration,
    ...(clientId ? { clientId } : {}),
    ...(clientMetadataUrl ? { clientMetadataUrl } : {}),
    ...(scopes ? { scopes } : {}),
    ...(endpoints ? { endpoints } : {}),
    ...(discoveredRaw !== undefined ? { discoveredAt: discoveredRaw } : {})
  }
  if (
    config.authMode === 'none' &&
    config.registrationMode === 'none' &&
    !config.clientId &&
    !config.clientMetadataUrl &&
    !config.scopes &&
    !config.endpoints &&
    config.discoveredAt === undefined
  ) {
    return undefined
  }
  return config
}

function readTimestamp(record: Record<string, unknown>, keys: string[]): number | undefined | null {
  for (const key of keys) {
    if (!Object.prototype.hasOwnProperty.call(record, key)) continue
    const value = record[key]
    if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) return null
    return value
  }
  return undefined
}

const TOP_LEVEL_OAUTH_SECRET_KEYS = new Set([
  'accessToken',
  'access_token',
  'refreshToken',
  'refresh_token',
  'clientSecret',
  'client_secret',
  'idToken',
  'id_token',
  'token'
])

function withoutTopLevelOAuthSecrets<T extends object>(server: T): T {
  const cleaned: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(server)) {
    if (TOP_LEVEL_OAUTH_SECRET_KEYS.has(key)) continue
    cleaned[key] = value
  }
  return cleaned as T
}

export function sanitizeMcpUpstream<T extends { type?: string; oauth?: unknown }>(server: T): T {
  const cleaned = withoutTopLevelOAuthSecrets(server)
  if (cleaned.type === undefined || cleaned.type === 'stdio' || cleaned.oauth === undefined) {
    if (!Object.prototype.hasOwnProperty.call(cleaned, 'oauth')) return cleaned
    const { oauth: _oauth, ...rest } = cleaned
    return rest as T
  }
  const oauth = normalizeOAuthConfig(cleaned.oauth)
  if (!oauth) {
    const { oauth: _oauth, ...rest } = cleaned
    return rest as T
  }
  return { ...cleaned, oauth }
}

function appendBearerAuthorization(
  headers: Array<{ name: string; value: string }> | undefined,
  bearerToken: unknown
): Array<{ name: string; value: string }> | undefined {
  if (typeof bearerToken !== 'string' || bearerToken.trim().length === 0) return headers
  const next = headers ? [...headers] : []
  if (!next.some((pair) => pair.name.toLowerCase() === 'authorization')) {
    next.push({ name: 'Authorization', value: `Bearer ${bearerToken.trim()}` })
  }
  return next
}

function normalizeStoredServer(value: unknown): StoredMcpServer | null {
  if (!isRecord(value) || typeof value.id !== 'string' || typeof value.name !== 'string')
    return null
  if (value.enabled !== undefined && typeof value.enabled !== 'boolean') return null
  const type = value.type ?? 'stdio'
  if (type === 'stdio') {
    if (typeof value.command !== 'string') return null
    if (value.args !== undefined && !Array.isArray(value.args)) return null
    const args = value.args?.filter((item): item is string => typeof item === 'string')
    if (value.args !== undefined && args?.length !== value.args.length) return null
    const env = stringPairs(value.env)
    if (value.env !== undefined && env === undefined) return null
    const server: StoredMcpServer = {
      id: value.id,
      type: 'stdio',
      name: value.name,
      command: value.command,
      ...(args ? { args } : {}),
      ...(env ? { env } : {}),
      enabled: value.enabled ?? true
    }
    return validateMcpServer(server).valid ? server : null
  }
  if ((type === 'http' || type === 'sse') && typeof value.url === 'string') {
    const parsedHeaders = stringPairs(value.headers)
    if (value.headers !== undefined && parsedHeaders === undefined) return null
    const headers = appendBearerAuthorization(parsedHeaders, value.bearerToken)
    const oauth = value.oauth === undefined ? undefined : normalizeOAuthConfig(value.oauth)
    if (oauth === null) return null
    const server: StoredMcpServer = {
      id: value.id,
      type,
      name: value.name,
      url: value.url,
      ...(headers ? { headers } : {}),
      ...(oauth ? { oauth } : {}),
      enabled: value.enabled ?? true
    }
    return validateMcpServer(server).valid ? server : null
  }
  return null
}

function extractUpstreamEntries(value: unknown): unknown[] | null {
  if (Array.isArray(value)) return value
  if (
    isRecord(value) &&
    (typeof value.schemaVersion === 'number' || Array.isArray(value.upstreams))
  ) {
    return Array.isArray(value.upstreams) ? value.upstreams : []
  }
  return null
}

export function normalizeMcpRegistry(value: unknown): StoredMcpServer[] {
  const entries = extractUpstreamEntries(value)
  if (entries == null) return []
  const normalized = entries.flatMap((entry) => {
    const server = normalizeStoredServer(entry)
    return server ? [server] : []
  })
  if (normalized.length !== entries.length) {
    console.warn(`[mcp] discarded ${entries.length - normalized.length} malformed registry entries`)
  }
  return normalized
}

export function buildMcpServers(registry: StoredMcpServer[], selectedIds: string[]): McpServer[] {
  const byId = new Map(registry.map((server) => [server.id, server]))
  return selectedIds.flatMap((id) => {
    const entry = byId.get(id)
    return entry ? [toWireServer(entry)] : []
  })
}

export function selectMcpServersForAgent(
  registry: StoredMcpServer[],
  capabilities: AgentCapabilities | null | undefined
): McpServerSelection {
  const servers: McpServer[] = []
  const skipped: SkippedMcpServer[] = []
  const mcpCapabilities = capabilities?.mcpCapabilities
  const pending = capabilities == null

  for (const entry of registry) {
    if (entry.enabled === false) continue
    const transport = transportOf(entry)
    if (transport === 'stdio' || pending) {
      servers.push(toWireServer(entry))
    } else if (mcpCapabilities?.[transport] === true) {
      servers.push(toWireServer(entry))
    } else {
      skipped.push({ id: entry.id, name: entry.name, transport })
    }
  }

  return { servers, skipped, pending }
}

async function loadLegacyDesktopRegistry(): Promise<StoredMcpServer[]> {
  const res = await persistenceApi.read<unknown>(ACP_MCP_KEY)
  if (res.success) return normalizeMcpRegistry(res.data)
  if (res.code === 'KEY_NOT_FOUND') return []
  throw new Error(
    res.error ?? runtimeT('mcp', 'persistence.loadFailed', 'Failed to load MCP servers')
  )
}

export async function loadMcpServers(): Promise<StoredMcpServer[]> {
  if (isTauriContext()) {
    const project = await loadMcpRegistryFromProject()
    if (project.success) return normalizeMcpRegistry(project.data)
    if (project.code === 'MCP_REGISTRY_NOT_FOUND' || project.code === 'NO_ACTIVE_PROJECT_ROOT') {
      return loadLegacyDesktopRegistry()
    }
    throw new Error(
      project.error ?? runtimeT('mcp', 'persistence.loadFailed', 'Failed to load MCP servers')
    )
  }
  const res = await webServerMcpServers.get()
  if (res.success) return normalizeMcpRegistry(res.data)
  if (res.code === 'KEY_NOT_FOUND') return []
  throw new Error(
    res.error ?? runtimeT('mcp', 'persistence.loadFailed', 'Failed to load MCP servers')
  )
}

export async function saveMcpServers(list: StoredMcpServer[]): Promise<void> {
  const normalized = normalizeMcpRegistry(list)
  if (isTauriContext()) {
    const res = await syncMcpRegistryToProject(normalized)
    if (!res.success) {
      throw new Error(
        res.error ?? runtimeT('mcp', 'persistence.saveFailed', 'Failed to persist MCP servers')
      )
    }
    return
  }
  const res = await webServerMcpServers.put(normalized)
  if (!res.success) {
    throw new Error(
      res.error ?? runtimeT('mcp', 'persistence.saveFailed', 'Failed to persist MCP servers')
    )
  }
}

/**
 * Best-effort wrapper for `syncMcpRegistryToProject`: logs a failure via
 * `logFrontendError` (with the IpcResult error/code) and never throws. Kept for
 * the acp-store project-switch hook so switching projects cannot fail closed on
 * a registry write. Direct settings saves use `saveMcpServers` and are fatal.
 */
export async function syncMcpRegistryToProjectBestEffort(
  registry: StoredMcpServer[]
): Promise<void> {
  try {
    const result = await syncMcpRegistryToProject(registry)
    if (!result.success) {
      void logFrontendError({
        source: 'acp-mcp-persistence.syncMcpRegistryToProject',
        message: `MCP registry project-file sync failed (${result.error ?? result.code ?? 'unknown'})`
      })
    }
  } catch (err) {
    void logFrontendError({
      source: 'acp-mcp-persistence.syncMcpRegistryToProject',
      message: `MCP registry project-file sync failed (${String(err)})`
    })
  }
}
