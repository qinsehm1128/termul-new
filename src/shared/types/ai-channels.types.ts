/**
 * Runtime-neutral AI channel contracts.
 *
 * Persisted documents store channels, endpoints, model profiles, routes, and
 * opaque keyring references. They cannot represent an API key, bearer token,
 * or other secret. Provider I/O is intentionally not implemented here.
 */

import {
  assertSafeHttpUrl,
  CONTRACT_MAX_SAFE_INTEGER,
  contractFail,
  hasOwn,
  readClosedObject,
  rejectNulls,
  requireBoolean,
  requireCredentialRef,
  requireDisplayText,
  requireInteger,
  requireModelId,
  requireSafeId
} from './contract-guards'

export const AI_CHANNELS_SCHEMA_VERSION = 1 as const
export const AI_ANALYSIS_SCHEMA_VERSION = 1 as const
export const FX_CAPABILITY_SCHEMA_VERSION = 1 as const

export const AI_ID_MAX_LENGTH = 64
export const AI_DISPLAY_NAME_MAX_LENGTH = 128
export const AI_MODEL_ID_MAX_LENGTH = 256
export const AI_URL_MAX_LENGTH = 2048
export const AI_CREDENTIAL_REF_MAX_LENGTH = 256
export const AI_CHANNELS_MAX = 32
export const AI_MODELS_PER_CHANNEL_MAX = 32
export const AI_PROFILES_MAX = 128
export const AI_ROUTE_PROFILES_MAX = 8
export const AI_MAX_ATTEMPTS_LIMIT = 8
export const AI_TIMEOUT_MS_MAX = 120_000
export const AI_MAX_OUTPUT_TOKENS_LIMIT = 1_000_000

const INVALID = 'AI contract document is invalid'
const FORBIDDEN = 'AI contract contains a forbidden credential field'
const INVALID_PROVIDER = 'AI provider is invalid'
const INVALID_ENDPOINT = 'AI endpoint is invalid'
const INVALID_ID = 'AI id is invalid'

export const AI_PROVIDER_KINDS = [
  'vercelGateway',
  'openAiCompatible',
  'openRouter',
  'ollama',
  'vllm',
  'custom'
] as const

export type AiProviderKind = (typeof AI_PROVIDER_KINDS)[number]

export const AI_PROVIDERS_REQUIRING_ENDPOINT: readonly AiProviderKind[] = [
  'openAiCompatible',
  'vllm',
  'custom'
]

export const AI_ROUTE_PURPOSES = ['descriptionAnalysis', 'fxRuntime'] as const
export type AiRoutePurpose = (typeof AI_ROUTE_PURPOSES)[number]

export const AI_CREDENTIAL_KINDS = ['keyring'] as const
export type AiCredentialKind = (typeof AI_CREDENTIAL_KINDS)[number]

export const AI_CREDENTIAL_STATES = ['present', 'missing', 'unavailable'] as const
export type AiCredentialState = (typeof AI_CREDENTIAL_STATES)[number]

export const AI_ANALYSIS_STATES = ['idle', 'running', 'draftReady', 'failed', 'cancelled'] as const
export type AiAnalysisState = (typeof AI_ANALYSIS_STATES)[number]

export const AI_ANALYSIS_ERROR_CODES = [
  'missingCredential',
  'invalidRoute',
  'timedOut',
  'cancelled',
  'providerRejected',
  'malformedOutput'
] as const
export type AiAnalysisErrorCode = (typeof AI_ANALYSIS_ERROR_CODES)[number]

export const FX_RUNTIME_KINDS = ['wasm', 'native', 'server', 'unavailable'] as const
export type FxRuntimeKind = (typeof FX_RUNTIME_KINDS)[number]

export interface AiCredentialRef {
  kind: 'keyring'
  ref: string
  hasCredential: boolean
}

export interface AiChannel {
  id: string
  displayName: string
  provider: AiProviderKind
  baseUrl?: string
  enabled: boolean
  credentialRef: AiCredentialRef
  modelIds: string[]
}

export interface AiModelCapabilities {
  descriptionAnalysis: boolean
  fxRuntime: boolean
  structuredOutput: boolean
  toolCalling: boolean
}

export interface AiModelProfile {
  id: string
  channelId: string
  modelId: string
  enabled: boolean
  capabilities: AiModelCapabilities
  maxOutputTokens?: number
  temperature?: number
}

export interface AiRoute {
  purpose: AiRoutePurpose
  profileIds: string[]
  maxAttempts: number
  timeoutMs: number
}

export interface AiChannelsDocument {
  schemaVersion: typeof AI_CHANNELS_SCHEMA_VERSION
  revision: number
  channels: AiChannel[]
  profiles: AiModelProfile[]
  routes: AiRoute[]
}

export interface AiCredentialStatus {
  channelId: string
  kind: 'keyring'
  ref: string
  hasCredential: boolean
  state: AiCredentialState
}

export interface AiAnalysisStatus {
  schemaVersion: typeof AI_ANALYSIS_SCHEMA_VERSION
  purpose: 'descriptionAnalysis'
  state: AiAnalysisState
  channelId?: string
  profileId?: string
  serverId?: string
  catalogRevision?: number
  draftRevision?: number
  errorCode?: AiAnalysisErrorCode
}

export interface FxCapabilityStatus {
  schemaVersion: typeof FX_CAPABILITY_SCHEMA_VERSION
  wasm: boolean
  jspi: boolean
  native: boolean
  server: boolean
  selectedRuntime: FxRuntimeKind
  fallback: 'aiRouter'
}

export interface AiRevisionFence {
  lastAccepted: number | null
}

export function createAiRevisionFence(): AiRevisionFence {
  return { lastAccepted: null }
}

export function acceptAiRevision(fence: AiRevisionFence, revision: number): void {
  if (!Number.isInteger(revision) || revision < 1 || revision > CONTRACT_MAX_SAFE_INTEGER) {
    contractFail(INVALID)
  }
  if (fence.lastAccepted !== null && revision <= fence.lastAccepted) {
    contractFail('AI contract revision is stale')
  }
  fence.lastAccepted = revision
}

export function parseAiChannelsDocument(value: unknown): AiChannelsDocument {
  rejectNulls(value, INVALID, FORBIDDEN)
  const record = readClosedObject(
    value,
    ['schemaVersion', 'revision', 'channels', 'profiles', 'routes'],
    INVALID,
    FORBIDDEN
  )
  if (record.schemaVersion !== AI_CHANNELS_SCHEMA_VERSION) contractFail(INVALID)
  const revision = requireInteger(record.revision, 1, CONTRACT_MAX_SAFE_INTEGER, INVALID)
  if (!Array.isArray(record.channels) || record.channels.length > AI_CHANNELS_MAX) {
    contractFail(INVALID)
  }
  if (!Array.isArray(record.profiles) || record.profiles.length > AI_PROFILES_MAX) {
    contractFail(INVALID)
  }
  if (!Array.isArray(record.routes)) contractFail(INVALID)

  const channels = record.channels.map(parseChannel)
  const channelIds = new Set<string>()
  const modelsByChannel = new Map<string, Set<string>>()
  for (const channel of channels) {
    if (channelIds.has(channel.id)) contractFail(INVALID_ID)
    channelIds.add(channel.id)
    modelsByChannel.set(channel.id, new Set(channel.modelIds))
  }

  const profiles = record.profiles.map((entry) => parseProfile(entry, channelIds, modelsByChannel))
  const profileIds = new Set<string>()
  for (const profile of profiles) {
    if (profileIds.has(profile.id)) contractFail(INVALID_ID)
    profileIds.add(profile.id)
  }

  const routes = record.routes.map((entry) => parseRoute(entry, profileIds))
  const purposes = new Set<string>()
  for (const route of routes) {
    if (purposes.has(route.purpose)) contractFail(INVALID)
    purposes.add(route.purpose)
  }

  return { schemaVersion: 1, revision, channels, profiles, routes }
}

export function serializeAiChannelsDocument(document: AiChannelsDocument): AiChannelsDocument {
  return {
    schemaVersion: document.schemaVersion,
    revision: document.revision,
    channels: document.channels.map((channel) => {
      const serialized: AiChannel = {
        id: channel.id,
        displayName: channel.displayName,
        provider: channel.provider,
        enabled: channel.enabled,
        credentialRef: { ...channel.credentialRef },
        modelIds: [...channel.modelIds]
      }
      if (channel.baseUrl !== undefined) serialized.baseUrl = channel.baseUrl
      return serialized
    }),
    profiles: document.profiles.map((profile) => {
      const serialized: AiModelProfile = {
        id: profile.id,
        channelId: profile.channelId,
        modelId: profile.modelId,
        enabled: profile.enabled,
        capabilities: { ...profile.capabilities }
      }
      if (profile.maxOutputTokens !== undefined)
        serialized.maxOutputTokens = profile.maxOutputTokens
      if (profile.temperature !== undefined) serialized.temperature = profile.temperature
      return serialized
    }),
    routes: document.routes.map((route) => ({
      purpose: route.purpose,
      profileIds: [...route.profileIds],
      maxAttempts: route.maxAttempts,
      timeoutMs: route.timeoutMs
    }))
  }
}

export function parseAiCredentialStatus(value: unknown): AiCredentialStatus {
  rejectNulls(value, INVALID, FORBIDDEN)
  const record = readClosedObject(
    value,
    ['channelId', 'kind', 'ref', 'hasCredential', 'state'],
    INVALID,
    FORBIDDEN
  )
  const channelId = requireSafeId(record.channelId, AI_ID_MAX_LENGTH, INVALID_ID)
  if (record.kind !== 'keyring') contractFail(INVALID)
  const ref = requireCredentialRef(record.ref, AI_CREDENTIAL_REF_MAX_LENGTH, INVALID)
  const hasCredential = requireBoolean(record.hasCredential, INVALID)
  const state = requireEnum(record.state, AI_CREDENTIAL_STATES, INVALID)
  if (hasCredential !== (state === 'present')) contractFail(INVALID)
  return { channelId, kind: 'keyring', ref, hasCredential, state }
}

export function parseAiAnalysisStatus(value: unknown): AiAnalysisStatus {
  rejectNulls(value, INVALID, FORBIDDEN)
  const record = readClosedObject(
    value,
    [
      'schemaVersion',
      'purpose',
      'state',
      'channelId',
      'profileId',
      'serverId',
      'catalogRevision',
      'draftRevision',
      'errorCode'
    ],
    INVALID,
    FORBIDDEN
  )
  if (record.schemaVersion !== AI_ANALYSIS_SCHEMA_VERSION) contractFail(INVALID)
  if (record.purpose !== 'descriptionAnalysis') contractFail(INVALID)
  const state = requireEnum(record.state, AI_ANALYSIS_STATES, INVALID)
  const status: AiAnalysisStatus = { schemaVersion: 1, purpose: 'descriptionAnalysis', state }
  if (hasOwn(record, 'channelId')) {
    status.channelId = requireSafeId(record.channelId, AI_ID_MAX_LENGTH, INVALID_ID)
  }
  if (hasOwn(record, 'profileId')) {
    status.profileId = requireSafeId(record.profileId, AI_ID_MAX_LENGTH, INVALID_ID)
  }
  if (hasOwn(record, 'serverId')) {
    status.serverId = requireSafeId(record.serverId, AI_ID_MAX_LENGTH, INVALID_ID)
  }
  if (hasOwn(record, 'catalogRevision')) {
    status.catalogRevision = requireInteger(
      record.catalogRevision,
      1,
      CONTRACT_MAX_SAFE_INTEGER,
      INVALID
    )
  }
  if (hasOwn(record, 'draftRevision')) {
    status.draftRevision = requireInteger(
      record.draftRevision,
      1,
      CONTRACT_MAX_SAFE_INTEGER,
      INVALID
    )
  }
  if (hasOwn(record, 'errorCode')) {
    status.errorCode = requireEnum(record.errorCode, AI_ANALYSIS_ERROR_CODES, INVALID)
  }
  if (state === 'idle' && (status.errorCode !== undefined || status.draftRevision !== undefined)) {
    contractFail(INVALID)
  }
  if ((state === 'failed' || state === 'cancelled') && status.errorCode === undefined) {
    contractFail(INVALID)
  }
  if (state === 'cancelled' && status.errorCode !== 'cancelled') contractFail(INVALID)
  if (state !== 'cancelled' && status.errorCode === 'cancelled') contractFail(INVALID)
  if (state === 'draftReady' && status.draftRevision === undefined) contractFail(INVALID)
  return status
}

export function serializeAiAnalysisStatus(status: AiAnalysisStatus): AiAnalysisStatus {
  const serialized: AiAnalysisStatus = {
    schemaVersion: status.schemaVersion,
    purpose: status.purpose,
    state: status.state
  }
  if (status.channelId !== undefined) serialized.channelId = status.channelId
  if (status.profileId !== undefined) serialized.profileId = status.profileId
  if (status.serverId !== undefined) serialized.serverId = status.serverId
  if (status.catalogRevision !== undefined) serialized.catalogRevision = status.catalogRevision
  if (status.draftRevision !== undefined) serialized.draftRevision = status.draftRevision
  if (status.errorCode !== undefined) serialized.errorCode = status.errorCode
  return serialized
}

export function parseFxCapabilityStatus(value: unknown): FxCapabilityStatus {
  rejectNulls(value, INVALID, FORBIDDEN)
  const record = readClosedObject(
    value,
    ['schemaVersion', 'wasm', 'jspi', 'native', 'server', 'selectedRuntime', 'fallback'],
    INVALID,
    FORBIDDEN
  )
  if (record.schemaVersion !== FX_CAPABILITY_SCHEMA_VERSION) contractFail(INVALID)
  if (record.fallback !== 'aiRouter') contractFail(INVALID)
  const status: FxCapabilityStatus = {
    schemaVersion: 1,
    wasm: requireBoolean(record.wasm, INVALID),
    jspi: requireBoolean(record.jspi, INVALID),
    native: requireBoolean(record.native, INVALID),
    server: requireBoolean(record.server, INVALID),
    selectedRuntime: requireEnum(record.selectedRuntime, FX_RUNTIME_KINDS, INVALID),
    fallback: 'aiRouter'
  }
  if (status.selectedRuntime === 'wasm' && !status.wasm) contractFail(INVALID)
  if (status.selectedRuntime === 'native' && !status.native) contractFail(INVALID)
  if (status.selectedRuntime === 'server' && !status.server) contractFail(INVALID)
  return status
}

function parseChannel(value: unknown): AiChannel {
  const record = readClosedObject(
    value,
    ['id', 'displayName', 'provider', 'baseUrl', 'enabled', 'credentialRef', 'modelIds'],
    INVALID,
    FORBIDDEN
  )
  const provider = requireEnum(record.provider, AI_PROVIDER_KINDS, INVALID_PROVIDER)
  const channel: AiChannel = {
    id: requireSafeId(record.id, AI_ID_MAX_LENGTH, INVALID_ID),
    displayName: requireDisplayText(record.displayName, AI_DISPLAY_NAME_MAX_LENGTH, INVALID),
    provider,
    enabled: requireBoolean(record.enabled, INVALID),
    credentialRef: parseCredentialRef(record.credentialRef),
    modelIds: parseModelIds(record.modelIds)
  }
  if (hasOwn(record, 'baseUrl')) {
    if (typeof record.baseUrl !== 'string') contractFail(INVALID_ENDPOINT)
    assertSafeHttpUrl(record.baseUrl, AI_URL_MAX_LENGTH, INVALID_ENDPOINT)
    channel.baseUrl = record.baseUrl
  } else if (AI_PROVIDERS_REQUIRING_ENDPOINT.includes(provider)) {
    contractFail(INVALID_ENDPOINT)
  }
  return channel
}

function parseCredentialRef(value: unknown): AiCredentialRef {
  const record = readClosedObject(value, ['kind', 'ref', 'hasCredential'], INVALID, FORBIDDEN)
  if (record.kind !== 'keyring') contractFail(INVALID)
  return {
    kind: 'keyring',
    ref: requireCredentialRef(record.ref, AI_CREDENTIAL_REF_MAX_LENGTH, INVALID),
    hasCredential: requireBoolean(record.hasCredential, INVALID)
  }
}

function parseModelIds(value: unknown): string[] {
  if (!Array.isArray(value) || value.length > AI_MODELS_PER_CHANNEL_MAX) contractFail(INVALID)
  const ids = value.map((entry) => requireModelId(entry, AI_MODEL_ID_MAX_LENGTH, INVALID_ID))
  if (new Set(ids).size !== ids.length) contractFail(INVALID_ID)
  return ids
}

function parseProfile(
  value: unknown,
  channelIds: Set<string>,
  modelsByChannel: Map<string, Set<string>>
): AiModelProfile {
  const record = readClosedObject(
    value,
    ['id', 'channelId', 'modelId', 'enabled', 'capabilities', 'maxOutputTokens', 'temperature'],
    INVALID,
    FORBIDDEN
  )
  const channelId = requireSafeId(record.channelId, AI_ID_MAX_LENGTH, INVALID_ID)
  const modelId = requireModelId(record.modelId, AI_MODEL_ID_MAX_LENGTH, INVALID_ID)
  if (!channelIds.has(channelId) || !modelsByChannel.get(channelId)?.has(modelId)) {
    contractFail(INVALID)
  }
  const profile: AiModelProfile = {
    id: requireSafeId(record.id, AI_ID_MAX_LENGTH, INVALID_ID),
    channelId,
    modelId,
    enabled: requireBoolean(record.enabled, INVALID),
    capabilities: parseCapabilities(record.capabilities)
  }
  if (hasOwn(record, 'maxOutputTokens')) {
    profile.maxOutputTokens = requireInteger(
      record.maxOutputTokens,
      1,
      AI_MAX_OUTPUT_TOKENS_LIMIT,
      INVALID
    )
  }
  if (hasOwn(record, 'temperature')) {
    profile.temperature = requireTemperature(record.temperature)
  }
  return profile
}

function parseCapabilities(value: unknown): AiModelCapabilities {
  const record = readClosedObject(
    value,
    ['descriptionAnalysis', 'fxRuntime', 'structuredOutput', 'toolCalling'],
    INVALID,
    FORBIDDEN
  )
  return {
    descriptionAnalysis: requireBoolean(record.descriptionAnalysis, INVALID),
    fxRuntime: requireBoolean(record.fxRuntime, INVALID),
    structuredOutput: requireBoolean(record.structuredOutput, INVALID),
    toolCalling: requireBoolean(record.toolCalling, INVALID)
  }
}

function parseRoute(value: unknown, profileIds: Set<string>): AiRoute {
  const record = readClosedObject(
    value,
    ['purpose', 'profileIds', 'maxAttempts', 'timeoutMs'],
    INVALID,
    FORBIDDEN
  )
  if (!Array.isArray(record.profileIds) || record.profileIds.length === 0) contractFail(INVALID)
  if (record.profileIds.length > AI_ROUTE_PROFILES_MAX) contractFail(INVALID)
  const ids = record.profileIds.map((entry) => requireSafeId(entry, AI_ID_MAX_LENGTH, INVALID_ID))
  if (new Set(ids).size !== ids.length) contractFail(INVALID_ID)
  for (const id of ids) {
    if (!profileIds.has(id)) contractFail(INVALID)
  }
  return {
    purpose: requireEnum(record.purpose, AI_ROUTE_PURPOSES, INVALID),
    profileIds: ids,
    maxAttempts: requireInteger(record.maxAttempts, 1, AI_MAX_ATTEMPTS_LIMIT, INVALID),
    timeoutMs: requireInteger(record.timeoutMs, 1, AI_TIMEOUT_MS_MAX, INVALID)
  }
}

function requireTemperature(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0 || value > 2) {
    contractFail(INVALID)
  }
  return value
}

function requireEnum<T extends string>(value: unknown, allowed: readonly T[], message: string): T {
  if (typeof value !== 'string' || !allowed.includes(value as T)) contractFail(message)
  return value as T
}
