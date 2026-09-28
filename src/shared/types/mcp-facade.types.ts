/**
 * MCP facade catalog, isolated failures, untrusted analysis drafts, and the
 * project metadata overlay. Metadata supplements descriptions only; it cannot
 * carry allow/deny or other safety authority, and it is not the MCP registry.
 */

import {
  CONTRACT_MAX_SAFE_INTEGER,
  contractFail,
  hasOwn,
  isForbiddenCredentialKey,
  isRecord,
  readClosedObject,
  rejectNulls,
  requireBoolean,
  requireDisplayText,
  requireInteger,
  requireSafeId,
  utf8ByteLength
} from './contract-guards'

export const MCP_FACADE_SCHEMA_VERSION = 1 as const
export const MCP_METADATA_SCHEMA_VERSION = 1 as const

export const MCP_SERVER_ID_MAX_LENGTH = 64
export const MCP_TOOL_NAME_MAX_LENGTH = 128
export const MCP_TEXT_MAX_CHARS = 4096
export const MCP_LIST_MAX = 32
export const MCP_TOOLS_MAX = 256
export const MCP_SCHEMA_MAX_BYTES = 65_536
export const MCP_FAILURES_MAX = 16
export const MCP_QUERY_MAX_CHARS = 256
export const MCP_FAILURE_MESSAGE_MAX_CHARS = 512

const FACADE_INVALID = 'MCP facade document is invalid'
const FACADE_FORBIDDEN = 'MCP facade contains a forbidden credential field'
const FACADE_ID = 'MCP facade id is invalid'
const FACADE_STALE = 'MCP facade catalog revision is stale'
const METADATA_INVALID = 'MCP metadata contract is invalid'
const METADATA_FORBIDDEN = 'MCP metadata contract contains a forbidden credential field'
const METADATA_ID = 'MCP metadata id is invalid'

export const MCP_FACADE_FAILURE_CODES = [
  'upstreamUnavailable',
  'schemaInvalid',
  'timedOut',
  'cancelled',
  'unauthorized'
] as const

export type McpFacadeFailureCode = (typeof MCP_FACADE_FAILURE_CODES)[number]

export interface McpFacadeTool {
  name: string
  description: string
  whenToUse: string
  avoidWhen: string
  inputSchema?: Record<string, unknown>
  readOnly: boolean
  destructive: boolean
  confirmationRequired: boolean
  allowed: boolean
}

export interface McpFacadeFailure {
  serverId: string
  code: McpFacadeFailureCode
  message: string
}

export interface McpFacadeCatalog {
  serverId: string
  catalogRevision: number
  description: string
  whenToUse: string[]
  avoidWhen: string[]
  tools: McpFacadeTool[]
  failures: McpFacadeFailure[]
}

export interface McpFacadeListQuery {
  query?: string
  limit?: number
  includeSchema: boolean
}

export interface McpFacadeToolCall {
  serverId: string
  toolName: string
  arguments: Record<string, unknown>
  catalogRevision?: number
}

export interface McpAnalysisDraftTool {
  name: string
  description: string
  whenToUse: string
  avoidWhen: string
  readOnly?: boolean
  destructive?: boolean
  confirmationRequired?: boolean
  confidence?: number
}

export interface McpAnalysisDraft {
  schemaVersion: typeof MCP_METADATA_SCHEMA_VERSION
  revision: number
  trust: 'untrusted'
  purpose: 'descriptionAnalysis'
  serverId: string
  catalogRevision: number
  channelId: string
  profileId: string
  description: string
  whenToUse: string[]
  avoidWhen: string[]
  tools: McpAnalysisDraftTool[]
}

export interface McpMetadataTool {
  name: string
  description: string
  whenToUse: string
  avoidWhen: string
}

export interface McpMetadataServer {
  serverId: string
  description: string
  whenToUse: string[]
  avoidWhen: string[]
  tools: McpMetadataTool[]
}

export interface McpMetadataDocument {
  schemaVersion: typeof MCP_METADATA_SCHEMA_VERSION
  revision: number
  servers: McpMetadataServer[]
}

export interface McpMetadataRevisionFence {
  lastAccepted: number | null
}

export function facadeToolNames(serverId: string): { list: string; call: string } {
  const id = requireSafeId(serverId, MCP_SERVER_ID_MAX_LENGTH, FACADE_ID)
  return { list: `${id}_tool_list`, call: `${id}_tool_call` }
}

export function requireCurrentCatalogRevision(
  requested: number | undefined,
  current: number
): void {
  if (!Number.isInteger(current) || current < 1 || current > CONTRACT_MAX_SAFE_INTEGER) {
    contractFail(FACADE_INVALID)
  }
  if (requested === undefined) return
  if (!Number.isInteger(requested) || requested < 1 || requested > CONTRACT_MAX_SAFE_INTEGER) {
    contractFail(FACADE_INVALID)
  }
  if (requested !== current) contractFail(FACADE_STALE)
}

export function parseMcpFacadeCatalog(value: unknown): McpFacadeCatalog {
  inspectFacadeValue(value, false)
  const record = readClosedObject(
    value,
    ['serverId', 'catalogRevision', 'description', 'whenToUse', 'avoidWhen', 'tools', 'failures'],
    FACADE_INVALID,
    FACADE_FORBIDDEN
  )
  const serverId = requireSafeId(record.serverId, MCP_SERVER_ID_MAX_LENGTH, FACADE_ID)
  const catalog: McpFacadeCatalog = {
    serverId,
    catalogRevision: requireInteger(
      record.catalogRevision,
      1,
      CONTRACT_MAX_SAFE_INTEGER,
      FACADE_INVALID
    ),
    description: requireProse(record.description, FACADE_INVALID),
    whenToUse: requireProseList(record.whenToUse, FACADE_INVALID),
    avoidWhen: requireProseList(record.avoidWhen, FACADE_INVALID),
    tools: parseTools(record.tools),
    failures: parseFailures(record.failures, serverId)
  }
  return catalog
}

export function serializeMcpFacadeCatalog(catalog: McpFacadeCatalog): McpFacadeCatalog {
  return {
    serverId: catalog.serverId,
    catalogRevision: catalog.catalogRevision,
    description: catalog.description,
    whenToUse: [...catalog.whenToUse],
    avoidWhen: [...catalog.avoidWhen],
    tools: catalog.tools.map((tool) => {
      const serialized: McpFacadeTool = {
        name: tool.name,
        description: tool.description,
        whenToUse: tool.whenToUse,
        avoidWhen: tool.avoidWhen,
        readOnly: tool.readOnly,
        destructive: tool.destructive,
        confirmationRequired: tool.confirmationRequired,
        allowed: tool.allowed
      }
      if (tool.inputSchema !== undefined) serialized.inputSchema = tool.inputSchema
      return serialized
    }),
    failures: catalog.failures.map((failure) => ({ ...failure }))
  }
}

export function parseMcpFacadeListQuery(value: unknown): McpFacadeListQuery {
  inspectFacadeValue(value, false)
  const record = readClosedObject(
    value,
    ['query', 'limit', 'includeSchema'],
    FACADE_INVALID,
    FACADE_FORBIDDEN
  )
  const query: McpFacadeListQuery = {
    includeSchema: hasOwn(record, 'includeSchema')
      ? requireBoolean(record.includeSchema, FACADE_INVALID)
      : false
  }
  if (hasOwn(record, 'query')) {
    query.query = requireDisplayText(record.query, MCP_QUERY_MAX_CHARS, FACADE_INVALID, {
      allowEmpty: true
    })
  }
  if (hasOwn(record, 'limit')) {
    query.limit = requireInteger(record.limit, 1, MCP_TOOLS_MAX, FACADE_INVALID)
  }
  return query
}

export function parseMcpFacadeToolCall(value: unknown): McpFacadeToolCall {
  inspectFacadeValue(value, false)
  const record = readClosedObject(
    value,
    ['serverId', 'toolName', 'arguments', 'catalogRevision'],
    FACADE_INVALID,
    FACADE_FORBIDDEN
  )
  if (!isRecord(record.arguments)) contractFail(FACADE_INVALID)
  assertSchemaSize(record.arguments, FACADE_INVALID)
  const call: McpFacadeToolCall = {
    serverId: requireSafeId(record.serverId, MCP_SERVER_ID_MAX_LENGTH, FACADE_ID),
    toolName: requireSafeId(record.toolName, MCP_TOOL_NAME_MAX_LENGTH, FACADE_ID),
    arguments: record.arguments
  }
  if (hasOwn(record, 'catalogRevision')) {
    call.catalogRevision = requireInteger(
      record.catalogRevision,
      1,
      CONTRACT_MAX_SAFE_INTEGER,
      FACADE_INVALID
    )
  }
  return call
}

export function parseMcpAnalysisDraft(value: unknown): McpAnalysisDraft {
  rejectNulls(value, METADATA_INVALID, METADATA_FORBIDDEN)
  const record = readClosedObject(
    value,
    [
      'schemaVersion',
      'revision',
      'trust',
      'purpose',
      'serverId',
      'catalogRevision',
      'channelId',
      'profileId',
      'description',
      'whenToUse',
      'avoidWhen',
      'tools'
    ],
    METADATA_INVALID,
    METADATA_FORBIDDEN
  )
  if (record.schemaVersion !== MCP_METADATA_SCHEMA_VERSION) contractFail(METADATA_INVALID)
  if (record.trust !== 'untrusted' || record.purpose !== 'descriptionAnalysis') {
    contractFail(METADATA_INVALID)
  }
  if (!Array.isArray(record.tools) || record.tools.length > MCP_TOOLS_MAX) {
    contractFail(METADATA_INVALID)
  }
  const tools = record.tools.map(parseDraftTool)
  assertUniqueNames(
    tools.map((tool) => tool.name),
    METADATA_ID
  )
  return {
    schemaVersion: 1,
    revision: requireInteger(record.revision, 1, CONTRACT_MAX_SAFE_INTEGER, METADATA_INVALID),
    trust: 'untrusted',
    purpose: 'descriptionAnalysis',
    serverId: requireSafeId(record.serverId, MCP_SERVER_ID_MAX_LENGTH, METADATA_ID),
    catalogRevision: requireInteger(
      record.catalogRevision,
      1,
      CONTRACT_MAX_SAFE_INTEGER,
      METADATA_INVALID
    ),
    channelId: requireSafeId(record.channelId, MCP_SERVER_ID_MAX_LENGTH, METADATA_ID),
    profileId: requireSafeId(record.profileId, MCP_SERVER_ID_MAX_LENGTH, METADATA_ID),
    description: requireProse(record.description, METADATA_INVALID),
    whenToUse: requireProseList(record.whenToUse, METADATA_INVALID),
    avoidWhen: requireProseList(record.avoidWhen, METADATA_INVALID),
    tools
  }
}

export function serializeMcpAnalysisDraft(draft: McpAnalysisDraft): McpAnalysisDraft {
  return {
    schemaVersion: draft.schemaVersion,
    revision: draft.revision,
    trust: draft.trust,
    purpose: draft.purpose,
    serverId: draft.serverId,
    catalogRevision: draft.catalogRevision,
    channelId: draft.channelId,
    profileId: draft.profileId,
    description: draft.description,
    whenToUse: [...draft.whenToUse],
    avoidWhen: [...draft.avoidWhen],
    tools: draft.tools.map((tool) => {
      const serialized: McpAnalysisDraftTool = {
        name: tool.name,
        description: tool.description,
        whenToUse: tool.whenToUse,
        avoidWhen: tool.avoidWhen
      }
      if (tool.readOnly !== undefined) serialized.readOnly = tool.readOnly
      if (tool.destructive !== undefined) serialized.destructive = tool.destructive
      if (tool.confirmationRequired !== undefined) {
        serialized.confirmationRequired = tool.confirmationRequired
      }
      if (tool.confidence !== undefined) serialized.confidence = tool.confidence
      return serialized
    })
  }
}

export function parseMcpMetadataDocument(value: unknown): McpMetadataDocument {
  rejectNulls(value, METADATA_INVALID, METADATA_FORBIDDEN)
  const record = readClosedObject(
    value,
    ['schemaVersion', 'revision', 'servers'],
    METADATA_INVALID,
    METADATA_FORBIDDEN
  )
  if (record.schemaVersion !== MCP_METADATA_SCHEMA_VERSION) contractFail(METADATA_INVALID)
  if (!Array.isArray(record.servers) || record.servers.length > MCP_TOOLS_MAX) {
    contractFail(METADATA_INVALID)
  }
  const servers = record.servers.map(parseMetadataServer)
  assertUniqueNames(
    servers.map((server) => server.serverId),
    METADATA_ID
  )
  return {
    schemaVersion: 1,
    revision: requireInteger(record.revision, 1, CONTRACT_MAX_SAFE_INTEGER, METADATA_INVALID),
    servers
  }
}

export function createMcpMetadataRevisionFence(): McpMetadataRevisionFence {
  return { lastAccepted: null }
}

export function acceptMcpMetadataRevision(fence: McpMetadataRevisionFence, revision: number): void {
  if (!Number.isInteger(revision) || revision < 1 || revision > CONTRACT_MAX_SAFE_INTEGER) {
    contractFail(METADATA_INVALID)
  }
  if (fence.lastAccepted !== null && revision <= fence.lastAccepted) {
    contractFail('MCP metadata revision is stale')
  }
  fence.lastAccepted = revision
}

function parseTools(value: unknown): McpFacadeTool[] {
  if (!Array.isArray(value) || value.length > MCP_TOOLS_MAX) contractFail(FACADE_INVALID)
  const tools = value.map(parseTool)
  assertUniqueNames(
    tools.map((tool) => tool.name),
    FACADE_ID
  )
  return tools
}

function parseTool(value: unknown): McpFacadeTool {
  const record = readClosedObject(
    value,
    [
      'name',
      'description',
      'whenToUse',
      'avoidWhen',
      'inputSchema',
      'readOnly',
      'destructive',
      'confirmationRequired',
      'allowed'
    ],
    FACADE_INVALID,
    FACADE_FORBIDDEN
  )
  const tool: McpFacadeTool = {
    name: requireSafeId(record.name, MCP_TOOL_NAME_MAX_LENGTH, FACADE_ID),
    description: requireProse(record.description, FACADE_INVALID),
    whenToUse: requireProse(record.whenToUse, FACADE_INVALID),
    avoidWhen: requireProse(record.avoidWhen, FACADE_INVALID),
    readOnly: requireBoolean(record.readOnly, FACADE_INVALID),
    destructive: requireBoolean(record.destructive, FACADE_INVALID),
    confirmationRequired: requireBoolean(record.confirmationRequired, FACADE_INVALID),
    allowed: requireBoolean(record.allowed, FACADE_INVALID)
  }
  if (hasOwn(record, 'inputSchema')) {
    if (!isRecord(record.inputSchema)) contractFail(FACADE_INVALID)
    assertSchemaSize(record.inputSchema, FACADE_INVALID)
    tool.inputSchema = record.inputSchema
  }
  return tool
}

function parseFailures(value: unknown, serverId: string): McpFacadeFailure[] {
  if (!Array.isArray(value) || value.length > MCP_FAILURES_MAX) contractFail(FACADE_INVALID)
  return value.map((entry) => {
    const record = readClosedObject(
      entry,
      ['serverId', 'code', 'message'],
      FACADE_INVALID,
      FACADE_FORBIDDEN
    )
    const failureServerId = requireSafeId(record.serverId, MCP_SERVER_ID_MAX_LENGTH, FACADE_ID)
    if (failureServerId !== serverId) contractFail(FACADE_INVALID)
    return {
      serverId: failureServerId,
      code: requireFailureCode(record.code),
      message: requireDisplayText(record.message, MCP_FAILURE_MESSAGE_MAX_CHARS, FACADE_INVALID, {
        allowEmpty: true
      })
    }
  })
}

function parseDraftTool(value: unknown): McpAnalysisDraftTool {
  const record = readClosedObject(
    value,
    [
      'name',
      'description',
      'whenToUse',
      'avoidWhen',
      'readOnly',
      'destructive',
      'confirmationRequired',
      'confidence'
    ],
    METADATA_INVALID,
    METADATA_FORBIDDEN
  )
  const tool: McpAnalysisDraftTool = {
    name: requireSafeId(record.name, MCP_TOOL_NAME_MAX_LENGTH, METADATA_ID),
    description: requireProse(record.description, METADATA_INVALID),
    whenToUse: requireProse(record.whenToUse, METADATA_INVALID),
    avoidWhen: requireProse(record.avoidWhen, METADATA_INVALID)
  }
  if (hasOwn(record, 'readOnly')) tool.readOnly = requireBoolean(record.readOnly, METADATA_INVALID)
  if (hasOwn(record, 'destructive')) {
    tool.destructive = requireBoolean(record.destructive, METADATA_INVALID)
  }
  if (hasOwn(record, 'confirmationRequired')) {
    tool.confirmationRequired = requireBoolean(record.confirmationRequired, METADATA_INVALID)
  }
  if (hasOwn(record, 'confidence')) tool.confidence = requireConfidence(record.confidence)
  return tool
}

function parseMetadataServer(value: unknown): McpMetadataServer {
  const record = readClosedObject(
    value,
    ['serverId', 'description', 'whenToUse', 'avoidWhen', 'tools'],
    METADATA_INVALID,
    METADATA_FORBIDDEN
  )
  if (!Array.isArray(record.tools) || record.tools.length > MCP_TOOLS_MAX) {
    contractFail(METADATA_INVALID)
  }
  const tools = record.tools.map(parseMetadataTool)
  assertUniqueNames(
    tools.map((tool) => tool.name),
    METADATA_ID
  )
  return {
    serverId: requireSafeId(record.serverId, MCP_SERVER_ID_MAX_LENGTH, METADATA_ID),
    description: requireProse(record.description, METADATA_INVALID),
    whenToUse: requireProseList(record.whenToUse, METADATA_INVALID),
    avoidWhen: requireProseList(record.avoidWhen, METADATA_INVALID),
    tools
  }
}

function parseMetadataTool(value: unknown): McpMetadataTool {
  const record = readClosedObject(
    value,
    ['name', 'description', 'whenToUse', 'avoidWhen'],
    METADATA_INVALID,
    METADATA_FORBIDDEN
  )
  return {
    name: requireSafeId(record.name, MCP_TOOL_NAME_MAX_LENGTH, METADATA_ID),
    description: requireProse(record.description, METADATA_INVALID),
    whenToUse: requireProse(record.whenToUse, METADATA_INVALID),
    avoidWhen: requireProse(record.avoidWhen, METADATA_INVALID)
  }
}

function requireProse(value: unknown, message: string): string {
  return requireDisplayText(value, MCP_TEXT_MAX_CHARS, message, {
    allowEmpty: true,
    allowNewlines: true
  })
}

function requireProseList(value: unknown, message: string): string[] {
  if (!Array.isArray(value) || value.length > MCP_LIST_MAX) contractFail(message)
  return value.map((entry) => requireProse(entry, message))
}

function requireFailureCode(value: unknown): McpFacadeFailureCode {
  if (
    typeof value !== 'string' ||
    !MCP_FACADE_FAILURE_CODES.includes(value as McpFacadeFailureCode)
  ) {
    contractFail(FACADE_INVALID)
  }
  return value as McpFacadeFailureCode
}

function requireConfidence(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0 || value > 1) {
    contractFail(METADATA_INVALID)
  }
  return value
}

function assertUniqueNames(names: string[], message: string): void {
  if (new Set(names).size !== names.length) contractFail(message)
}

function assertSchemaSize(value: Record<string, unknown>, message: string): void {
  const bytes = utf8ByteLength(JSON.stringify(value))
  if (bytes > MCP_SCHEMA_MAX_BYTES) contractFail(message)
}

function inspectFacadeValue(value: unknown, inSchema: boolean, depth = 0): void {
  if (depth > 40) contractFail(FACADE_INVALID)
  if (value === null) {
    if (inSchema) return
    contractFail(FACADE_INVALID)
  }
  if (typeof value === 'string') {
    if (value.length > 100_000) contractFail(FACADE_INVALID)
    return
  }
  if (Array.isArray(value)) {
    if (value.length > 10_000) contractFail(FACADE_INVALID)
    for (const item of value) inspectFacadeValue(item, inSchema, depth + 1)
    return
  }
  if (!isRecord(value)) return
  if (Object.keys(value).length > 256 && !inSchema) contractFail(FACADE_INVALID)
  for (const [key, child] of Object.entries(value)) {
    if (!inSchema && isForbiddenCredentialKey(key)) contractFail(FACADE_FORBIDDEN)
    const childInSchema = inSchema || key === 'inputSchema' || key === 'arguments'
    inspectFacadeValue(child, childInSchema, depth + 1)
  }
}
