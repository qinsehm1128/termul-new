import { runtimeT } from '@/i18n/runtime'
import type { McpEnvVar, McpServerConfig } from '@/lib/acp-api'
import { validateMcpServer } from '@/lib/acp-mcp-persistence'

export interface McpJsonImportResult {
  servers: McpServerConfig[]
  errors: string[]
}

const TRANSPORTS = ['stdio', 'http', 'sse'] as const
type McpTransport = (typeof TRANSPORTS)[number]

type ImportEntry = [name: string, value: unknown]

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function hasOwn(value: Record<string, unknown>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(value, key)
}

function stripOuterQuotes(value: string): string {
  const trimmed = value.trim()
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1).trim()
  }
  return trimmed
}

/** All-string array, or `undefined` when `value` is not a string array. */
function stringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) return undefined
  const strings = value.filter((item): item is string => typeof item === 'string')
  return strings.length === value.length ? strings : undefined
}

/**
 * Normalize the argv forms emitted by Claude Desktop and tauri-mcp-router.
 * The router also accepts a whitespace-delimited string, while the existing
 * Termul editor uses arrays, so both are kept import-compatible.
 */
function normalizeArgs(value: unknown): string[] | undefined {
  if (value === undefined) return undefined
  if (typeof value === 'string') {
    return value
      .trim()
      .split(/\s+/)
      .filter((arg) => arg.length > 0)
      .map(stripOuterQuotes)
  }
  return stringArray(value)?.map(stripOuterQuotes)
}

/**
 * Normalize a map or `[{name, value}]` list into Termul's pair shape.
 *
 * Canonical control-plane files may contain `{name, ref}` entries for a secret
 * reference. The renderer's import shape only supports inline values, so such
 * entries are intentionally omitted; inline values remain untouched. This is
 * the only secret-dropping path and it is driven by an explicit `ref` field.
 */
function normalizePairs(value: unknown): McpEnvVar[] | undefined {
  if (value === undefined) return undefined
  if (Array.isArray(value)) {
    const pairs: McpEnvVar[] = []
    for (const entry of value) {
      if (!isRecord(entry) || typeof entry.name !== 'string') return undefined
      if (typeof entry.ref === 'string' && entry.ref.trim().length > 0) continue
      if (typeof entry.value === 'string') {
        pairs.push({ name: entry.name, value: entry.value })
        continue
      }
      return undefined
    }
    return pairs
  }
  if (isRecord(value)) {
    const pairs: McpEnvVar[] = []
    for (const [name, entryValue] of Object.entries(value)) {
      if (isRecord(entryValue)) {
        if (typeof entryValue.ref === 'string' && entryValue.ref.trim().length > 0) continue
        if (typeof entryValue.value === 'string') {
          pairs.push({ name, value: entryValue.value })
          continue
        }
        return undefined
      }
      pairs.push({ name, value: String(entryValue) })
    }
    return pairs
  }
  return undefined
}

/**
 * Normalize `env` from a Claude Desktop-style `Record<string,string>` map into
 * Se's internal `[{name, value}]` shape. An already-normalized array passes
 * through after rebuilding fresh pair objects so unknown fields are dropped.
 */
function normalizeEnv(value: unknown): McpEnvVar[] | undefined {
  return normalizePairs(value)
}

function isHttpUrl(value: unknown): value is string {
  return typeof value === 'string' && /^https?:\/\//i.test(stripOuterQuotes(value))
}

function isMcpProxyCommand(command: unknown): boolean {
  if (typeof command !== 'string') return false
  const normalized = stripOuterQuotes(command).replace(/\\/g, '/').toLowerCase()
  return normalized.endsWith('/mcp-proxy') || normalized.endsWith('mcp-proxy')
}

function findRemoteUrlFromArgs(args: string[] | undefined): string | undefined {
  if (!args) return undefined
  const flagIndex = args.findIndex((arg) => arg === '--url' || arg === '-u')
  const flagged = flagIndex >= 0 ? args[flagIndex + 1] : undefined
  if (isHttpUrl(flagged)) return stripOuterQuotes(flagged)
  const found = args.find((arg) => isHttpUrl(arg))
  return found ? stripOuterQuotes(found) : undefined
}

function firstStringField(
  raw: Record<string, unknown>,
  fields: string[]
): { value?: string; invalid: boolean } {
  for (const field of fields) {
    if (!hasOwn(raw, field)) continue
    const value = raw[field]
    if (typeof value !== 'string') return { invalid: true }
    const trimmed = value.trim()
    if (trimmed.length > 0) return { value: stripOuterQuotes(trimmed), invalid: false }
  }
  return { invalid: false }
}

function extractRemoteUrl(
  raw: Record<string, unknown>,
  args: string[] | undefined
): string | undefined {
  const configured = firstStringField(raw, ['remoteUrl', 'remote_url', 'serverUrl', 'url'])
  if (configured.value) return configured.value
  if (configured.invalid) return undefined

  if (isHttpUrl(raw.command)) return stripOuterQuotes(raw.command)
  if (isMcpProxyCommand(raw.command)) return findRemoteUrlFromArgs(args)
  return args?.some((arg) => isMcpProxyCommand(arg)) ? findRemoteUrlFromArgs(args) : undefined
}

function declaredTransport(raw: Record<string, unknown>): McpTransport | undefined {
  const declared = raw.type ?? raw.serverType ?? raw.server_type
  if (typeof declared !== 'string') return undefined
  switch (declared.trim().toLowerCase()) {
    case 'stdio':
    case 'local':
      return 'stdio'
    case 'sse':
      return 'sse'
    case 'http':
    case 'https':
    case 'remote':
    case 'remote-streamable':
    case 'streamable-http':
    case 'streamable_http':
      return 'http'
    default:
      return undefined
  }
}

function upsertPair(pairs: McpEnvVar[], next: McpEnvVar): void {
  const key = next.name.toLowerCase()
  const existingIndex = pairs.findIndex((pair) => pair.name.toLowerCase() === key)
  if (existingIndex >= 0) pairs[existingIndex] = next
  else pairs.push(next)
}

function collectRemoteHeaders(
  raw: Record<string, unknown>,
  env: McpEnvVar[] | undefined
): { headers?: McpEnvVar[]; invalid: boolean } {
  const headers: McpEnvVar[] = []
  if (env) {
    for (const pair of env) upsertPair(headers, pair)
  }

  const sources: unknown[] = []
  if (hasOwn(raw, 'http_headers')) sources.push(raw.http_headers)
  if (hasOwn(raw, 'request_options')) {
    const requestOptions = raw.request_options
    if (isRecord(requestOptions) && hasOwn(requestOptions, 'headers')) {
      sources.push(requestOptions.headers)
    }
  }
  if (hasOwn(raw, 'requestOptions')) {
    const requestOptions = raw.requestOptions
    if (isRecord(requestOptions) && hasOwn(requestOptions, 'headers')) {
      sources.push(requestOptions.headers)
    }
  }
  if (hasOwn(raw, 'headers')) sources.push(raw.headers)

  for (const source of sources) {
    const pairs = normalizePairs(source)
    if (pairs === undefined) return { invalid: true }
    for (const pair of pairs) upsertPair(headers, pair)
  }

  const bearerToken = raw.bearerToken ?? raw.bearer_token
  if (typeof bearerToken === 'string' && bearerToken.trim().length > 0) {
    upsertPair(headers, { name: 'Authorization', value: `Bearer ${bearerToken.trim()}` })
  }

  return { headers: headers.length > 0 ? headers : undefined, invalid: false }
}

/**
 * Build a (partial) server config from a parsed entry, keeping only Termul's
 * known transport fields. Aliases used by Qin/tauri-mcp-router are normalized
 * here (`remoteUrl`, `remote_url`, `serverUrl`, `serverType`, and headers in
 * requestOptions/request_options).
 */
function buildServer(
  raw: Record<string, unknown>,
  name: string,
  env: McpEnvVar[] | undefined
): Partial<McpServerConfig> | null {
  const args = normalizeArgs(raw.args)
  if (raw.args !== undefined && args === undefined) return null

  const commandField = firstStringField(raw, ['command', 'script'])
  const command = commandField.value
  const remoteUrl = extractRemoteUrl(raw, args)
  const type = declaredTransport(raw) ?? (remoteUrl ? 'http' : command ? 'stdio' : 'stdio')

  if (type === 'stdio') {
    if (raw.command !== undefined && typeof raw.command !== 'string' && raw.script === undefined) {
      return null
    }
    if (raw.script !== undefined && typeof raw.script !== 'string') return null
    return {
      type: 'stdio',
      name,
      ...(command ? { command } : {}),
      ...(args && args.length > 0 ? { args } : {}),
      ...(env && env.length > 0 ? { env } : {})
    }
  }

  if (
    !remoteUrl &&
    (raw.url !== undefined || raw.remoteUrl !== undefined || raw.remote_url !== undefined)
  ) {
    return {
      type,
      name,
      ...(env && env.length > 0 ? { headers: env } : {})
    } as Partial<McpServerConfig>
  }

  const remoteHeaders = collectRemoteHeaders(raw, env)
  if (remoteHeaders.invalid) return null
  return {
    type,
    name,
    ...(remoteUrl ? { url: remoteUrl } : {}),
    ...(remoteHeaders.headers ? { headers: remoteHeaders.headers } : {})
  } as Partial<McpServerConfig>
}

function entriesFromWrapper(value: unknown): ImportEntry[] | null {
  if (!isRecord(value)) return null
  return Object.entries(value)
}

function entriesFromArray(value: unknown[]): ImportEntry[] {
  return value.map((entry) => [
    isRecord(entry) && typeof entry.name === 'string' ? entry.name : '',
    entry
  ])
}

function hasCanonicalMarker(value: Record<string, unknown>): boolean {
  return (
    hasOwn(value, 'schemaVersion') ||
    hasOwn(value, 'builtIns') ||
    hasOwn(value, 'upstreams') ||
    hasOwn(value, 'routing')
  )
}

function extractEntries(parsed: unknown): { entries: ImportEntry[]; error?: string } {
  if (Array.isArray(parsed)) return { entries: entriesFromArray(parsed) }
  if (!isRecord(parsed)) {
    return {
      entries: [],
      error: runtimeT('mcp', 'import.topLevelObject', 'Invalid JSON: expected a top-level object')
    }
  }

  // Canonical exports also contain `mcpServers` for compatibility. Prefer the
  // canonical upstream list so importing an export never duplicates each entry.
  if (hasCanonicalMarker(parsed)) {
    if (parsed.upstreams === undefined) return { entries: [] }
    if (!Array.isArray(parsed.upstreams)) {
      return {
        entries: [],
        error: runtimeT(
          'mcp',
          'import.upstreamsArray',
          'Invalid JSON: "upstreams" must be an array'
        )
      }
    }
    return { entries: entriesFromArray(parsed.upstreams) }
  }

  if (hasOwn(parsed, 'mcpServers')) {
    const entries = entriesFromWrapper(parsed.mcpServers)
    if (entries) return { entries }
    return {
      entries: [],
      error: runtimeT('mcp', 'import.serversObject', 'Invalid JSON: "mcpServers" must be an object')
    }
  }

  if (hasOwn(parsed, 'servers')) {
    if (Array.isArray(parsed.servers)) return { entries: entriesFromArray(parsed.servers) }
    const entries = entriesFromWrapper(parsed.servers)
    if (entries) return { entries }
    return {
      entries: [],
      error: runtimeT(
        'mcp',
        'import.serversShape',
        'Invalid JSON: "servers" must be an object or array'
      )
    }
  }

  // Bare single-server object.
  return { entries: [['', parsed]] }
}

function entryDisplayName(keyName: string, raw: unknown): string {
  if (keyName.length > 0) return keyName
  return isRecord(raw) && typeof raw.name === 'string' ? raw.name : ''
}

/**
 * Parse MCP JSON into saveable `McpServerConfig`s.
 *
 * Accepted shapes include Claude `mcpServers`, Qin/tauri-mcp-router `servers`
 * maps or arrays, a top-level server array, the canonical control-plane object,
 * and a bare single-server object. Unknown fields are dropped. Env/header maps
 * become `[{name, value}]` pairs and explicit secret references are omitted
 * because the renderer cannot resolve references without the host authority.
 * Each server is validated via `validateMcpServer`; invalid and duplicate
 * entries are reported per-server while valid entries still import.
 */
export function parseMcpJsonImport(text: string): McpJsonImportResult {
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch (error) {
    return {
      servers: [],
      errors: [
        runtimeT('mcp', 'import.invalidJson', `Invalid JSON: ${String(error)}`, {
          message: String(error)
        })
      ]
    }
  }

  const extracted = extractEntries(parsed)
  if (extracted.error) return { servers: [], errors: [extracted.error] }

  const servers: McpServerConfig[] = []
  const errors: string[] = []
  const seenNames = new Set<string>()

  for (const [keyName, raw] of extracted.entries) {
    const name = entryDisplayName(keyName, raw)
    if (!isRecord(raw)) {
      errors.push(
        runtimeT('mcp', 'import.serverObject', `${name || '<unknown>'}: expected a server object`, {
          name: name || runtimeT('mcp', 'import.unknown', '<unknown>')
        })
      )
      continue
    }

    const env = normalizeEnv(raw.env)
    if (raw.env !== undefined && env === undefined) {
      errors.push(
        runtimeT(
          'mcp',
          'import.envShape',
          `${name || '<unnamed>'}: env must be an object map or name/value pairs`,
          { name: name || runtimeT('mcp', 'import.unnamed', '<unnamed>') }
        )
      )
      continue
    }

    const server = buildServer(raw, name, env)
    if (!server) {
      errors.push(
        runtimeT(
          'mcp',
          'import.invalidServer',
          `${name || '<unnamed>'}: invalid server configuration`,
          { name: name || runtimeT('mcp', 'import.unnamed', '<unnamed>') }
        )
      )
      continue
    }

    const validation = validateMcpServer(server)
    if (!validation.valid) {
      errors.push(
        runtimeT(
          'mcp',
          'import.validation',
          `${name || '<unnamed>'}: ${validation.errors.join(' ')}`,
          {
            name: name || runtimeT('mcp', 'import.unnamed', '<unnamed>'),
            errors: validation.errors.join(' ')
          }
        )
      )
      continue
    }

    const normalized = server as McpServerConfig
    const nameKey = normalized.name.trim().toLocaleLowerCase()
    if (seenNames.has(nameKey)) {
      errors.push(
        runtimeT('mcp', 'import.duplicate', `${name || '<unnamed>'}: duplicate server skipped`, {
          name: name || runtimeT('mcp', 'import.unnamed', '<unnamed>')
        })
      )
      continue
    }
    seenNames.add(nameKey)
    servers.push(normalized)
  }

  return { servers, errors }
}
