/** Desktop ACP history persistence boundary. */

import { isConversationId } from '@shared/types/conversation.types'
import {
  assertConversationHistoryPage,
  assertConversationHistoryPageRequest,
  type ConversationHistoryPageV1,
  ConversationHistoryPageValidationError,
  type ConversationHistoryRecordV1,
  conversationHistoryPageEncodedBytes,
  type PersistedSessionSummary
} from '@shared/types/web-protocol.types'
import { runtimeT } from '@/i18n/runtime'
import type { ContentBlock, PlanEntry, SessionUsage, ToolCall } from '@/lib/acp-api'
import { acpHistoryApi } from '@/lib/acp-history-api'
import {
  AcpTransportError,
  getAcpTransport,
  isTransientAcpTransportError
} from '@/lib/acp-transport'
import { persistenceApi } from '@/lib/api'
import { conversationIdForIndexEntry } from '@/lib/conversation-binding'
import { logFrontendError } from '@/lib/log-api'
import type { ChatMessage, SessionStatus } from '@/stores/acp-store'

export const SESSION_INDEX_KEY = 'acp/sessions/index'
export const WIPE_MIGRATION_KEY = 'acp/sessions/migrated-v2'
export const INACTIVE_PAYLOAD_CACHE_BUDGET = 3
export const RENDERER_HISTORY_PAGE_SIZE = 250
export const MAX_HISTORY_IN_FLIGHT_BYTES = 4 * 1024 * 1024
export const MAX_FAILED_PREFIX_ASSEMBLIES = 4
export const MAX_FAILED_PREFIX_PAYLOAD_BYTES = 4_194_304
export const FAILED_PREFIX_TTL_MS = 120_000
export const MAX_RESUME_METADATA_ENTRIES = 32
export const RESUME_METADATA_TTL_MS = 1_800_000
export const RESUME_METADATA_MAX_BYTES = 8192

export function sessionPayloadKey(id: string): string {
  return `acp/sessions/${id}`
}

export interface SessionIndexEntry {
  id: string
  /** Canonical Conversation identity; host summaries expose it as storageKey. */
  conversationId?: string
  /** Present on some host list payloads; treated as Conversation identity when canonical. */
  storageKey?: string
  agentId: string
  agentConfigId?: string
  title: string
  cwd: string
  projectId: string
  createdAt: number
  lastActivityAt: number
  messageCount: number
  /**
   * Highest persisted message `seq` for this session (R3 / parent-spec R2
   * index-list completeness). Optional: absent on entries loaded from a Rust
   * index that does not yet surface it (degrades to 0, the pre-R3 value).
   * `get_session_cursor` remains the authoritative functional cursor.
   */
  lastSeq?: number
  status: SessionStatus
  /** Agent-owned metadata mirror created from ACP `session/list`; no local transcript. */
  discovered?: boolean
  /**
   * Worktree path + branch the agent runs in (CAP-3). Additive: absent on
   * pre-feature sessions. Powers the CAP-6 indicator + the deleted-worktree
   * fallback; state isolation still keys on `cwd`.
   */
  worktreePath?: string
  worktreeBranch?: string
}

export interface SessionPayload {
  metadata: SessionIndexEntry
  messages: ChatMessage[]
  /**
   * Durable tool calls mirrored alongside the transcript so history reopens
   * and post-reload resumes restore the tool cards in the timeline. Written
   * through `sanitizeToolCallsForPersistence` (no `rawOutput`, per-call size
   * bound). Absent on payloads persisted before this field existed.
   */
  toolCalls?: ToolCall[]
  /** Latest valid durable context-window snapshot. */
  sessionUsage?: SessionUsage
  /** Latest durable ACP plan replacement. */
  plan?: PlanEntry[]
}

export interface HistoryPageProgress {
  sessionId: string
  pageNumber: number
  pageRecordCount: number
  loadedRecordCount: number
  nextCursor: number
  targetLastSeq: number
  complete: boolean
  inFlightBytes: number
  resumed: boolean
}

export interface LoadSessionPayloadOptions {
  /** Store-owned index metadata avoids a redundant list request before page one. */
  metadata?: SessionIndexEntry
  /** Awaited before the next request whenever a bounded transcript snapshot is published. */
  onPage?: (payload: SessionPayload, progress: HistoryPageProgress) => void | Promise<void>
  /** Per-page cursor/count progress without rebuilding or copying the transcript prefix. */
  onProgress?: (progress: HistoryPageProgress) => void | Promise<void>
}

export class ConversationHistoryLoadError extends Error {
  readonly code: string

  constructor(code: string, message: string) {
    super(message)
    this.name = 'ConversationHistoryLoadError'
    this.code = code
  }
}

/**
 * Maximum serialized UTF-8 size of a single durable tool call. Calls exceeding
 * the budget degrade to the structural subset so a giant diff or tool input
 * cannot balloon the on-disk payload.
 */
export const PERSISTED_TOOL_CALL_BYTE_BUDGET = 32 * 1024

/**
 * Maximum number of tool calls persisted per session. Tool calls are never
 * trimmed in the live window (messages have `MAX_LIVE_WINDOW_MESSAGES`), so the
 * durable mirror keeps only the most recent calls — the same recency window a
 * reader browses — bounding payload growth on very long sessions.
 */
export const PERSISTED_TOOL_CALLS_LIMIT = 500

/** Agent-controlled titles are bounded so the degraded subset stays bounded. */
const PERSISTED_TOOL_CALL_TITLE_LIMIT = 200

const persistedTextEncoder = new TextEncoder()

function boundedTitle(title: unknown): string | undefined {
  if (typeof title !== 'string' || title.length === 0) return undefined
  return title.length > PERSISTED_TOOL_CALL_TITLE_LIMIT
    ? `${title.slice(0, PERSISTED_TOOL_CALL_TITLE_LIMIT)}…`
    : title
}

/**
 * Structural subset of a durable tool call: routing/status fields + timeline
 * stamps only. Mid-flight statuses are persisted as `failed` — the turn that
 * owned them has ended, and restoring `pending`/`in_progress` would reopen the
 * card spinning forever.
 */
function structuralToolCall(toolCall: ToolCall): ToolCall {
  const reduced: ToolCall = { toolCallId: toolCall.toolCallId }
  const title = boundedTitle(toolCall.title)
  if (title !== undefined) reduced.title = title
  if (toolCall.kind !== undefined) reduced.kind = toolCall.kind
  const status =
    toolCall.status === 'pending' || toolCall.status === 'in_progress' ? 'failed' : toolCall.status
  if (status !== undefined) reduced.status = status
  if (typeof toolCall.timestamp === 'number') reduced.timestamp = toolCall.timestamp
  if (typeof toolCall.seq === 'number') reduced.seq = toolCall.seq
  return reduced
}

/**
 * Mirror-ready tool calls for durable history. `rawOutput` (unbounded tool
 * results) is never persisted, and only the known summary/render fields
 * (`rawInput`, structured `content`) ride along — unknown agent fields are
 * dropped at the boundary. Over-budget calls degrade to the structural subset
 * instead of ballooning the payload; non-serializable calls degrade the same
 * way (and the degradation is logged, never silent).
 */
export function sanitizeToolCallsForPersistence(
  toolCalls: ToolCall[] | undefined
): ToolCall[] | undefined {
  if (!Array.isArray(toolCalls) || toolCalls.length === 0) return undefined
  const degraded: string[] = []
  const dropped: string[] = []
  const sanitized: ToolCall[] = []
  for (const toolCall of toolCalls.slice(-PERSISTED_TOOL_CALLS_LIMIT)) {
    const candidate = structuralToolCall(toolCall)
    if (toolCall.rawInput !== undefined) candidate.rawInput = toolCall.rawInput
    if (toolCall.content !== undefined) candidate.content = toolCall.content
    let persisted: ToolCall | undefined
    try {
      const bytes = persistedTextEncoder.encode(JSON.stringify(candidate)).byteLength
      if (bytes <= PERSISTED_TOOL_CALL_BYTE_BUDGET) persisted = candidate
    } catch {
      // Non-serializable fields: fall through to the structural subset.
    }
    if (!persisted) {
      // Over budget or non-serializable: degrade to the structural subset, then
      // re-measure it. `toolCallId` is agent-sourced and unbounded, so even the
      // subset can exceed the cap — omit the call entirely in that case so the
      // per-call budget actually holds.
      const structural = structuralToolCall(toolCall)
      const structuralBytes = persistedTextEncoder.encode(JSON.stringify(structural)).byteLength
      if (structuralBytes <= PERSISTED_TOOL_CALL_BYTE_BUDGET) {
        degraded.push(toolCall.toolCallId)
        persisted = structural
      } else {
        dropped.push(toolCall.toolCallId)
      }
    }
    if (persisted) sanitized.push(persisted)
  }
  if (degraded.length > 0 || dropped.length > 0) {
    const parts: string[] = []
    if (degraded.length > 0) {
      parts.push(`degraded ${degraded.length}: ${degraded.slice(0, 3).join(', ')}`)
    }
    if (dropped.length > 0) {
      parts.push(`dropped ${dropped.length} over-size: ${dropped.slice(0, 3).join(', ')}`)
    }
    void logFrontendError({
      level: 'warn',
      source: 'acp.historyPersistence',
      message: `Tool-call persistence budget enforced — ${parts.join('; ')}`
    })
  }
  return sanitized.length > 0 ? sanitized : undefined
}

/** True for a restorable tool-call record: a non-null object with a string id. */
function isRestorableToolCall(value: unknown): value is ToolCall {
  if (typeof value !== 'object' || value === null) return false
  const candidate = value as ToolCall
  return typeof candidate.toolCallId === 'string' && candidate.toolCallId.length > 0
}

/** Filter a raw payload array down to restorable tool-call records. */
function normalizedToolCalls(toolCalls: unknown): ToolCall[] {
  if (!Array.isArray(toolCalls)) return []
  return toolCalls.filter(isRestorableToolCall)
}

/**
 * Highest `seq` across a payload's messages and tool calls (they share one
 * timeline counter). Corrupt/partial payloads degrade to the fields present —
 * a non-array `toolCalls`, or one containing non-record entries (`null`,
 * scalar, missing id), never throws on the reopen hot path.
 */
export function maxPayloadSeq(payload: Pick<SessionPayload, 'messages' | 'toolCalls'>): number {
  let maxSeq = 0
  for (const message of payload.messages) {
    if (typeof message.seq === 'number' && Number.isFinite(message.seq) && message.seq > maxSeq) {
      maxSeq = message.seq
    }
  }
  for (const toolCall of normalizedToolCalls(payload.toolCalls)) {
    if (
      typeof toolCall.seq === 'number' &&
      Number.isFinite(toolCall.seq) &&
      toolCall.seq > maxSeq
    ) {
      maxSeq = toolCall.seq
    }
  }
  return maxSeq
}

/** Restored tool calls for a payload, tolerant of legacy/corrupt shapes. */
export function restoredToolCalls(payload: Pick<SessionPayload, 'toolCalls'>): ToolCall[] {
  return normalizedToolCalls(payload.toolCalls)
}

type JsonObject = Record<string, unknown>

function asJsonObject(value: unknown): JsonObject | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as JsonObject)
    : null
}

function asContentBlock(value: unknown): ContentBlock | null {
  const object = asJsonObject(value)
  return object && typeof object.type === 'string' ? (object as ContentBlock) : null
}

let historyTraversalStarts = 0
let historyPageRequests = 0
let historyPageApplications = 0
let historyRecordApplications = 0
let historyTranscriptEntriesCopied = 0
let historyToolEntriesCopied = 0
let historyToolIndexLookups = 0
let historySnapshotsCreated = 0

function cloneHistoryBlock(block: ContentBlock): ContentBlock {
  return { ...block } as ContentBlock
}

/** Mutate only the accumulator-private block array; published snapshots own cloned blocks. */
function appendHistoryBlock(existing: ContentBlock[], incoming: ContentBlock): void {
  if (incoming.type === 'text') {
    const last = existing.at(-1)
    if (last?.type === 'text') {
      last.text = (last.text ?? '') + (incoming.text ?? '')
      return
    }
  }
  existing.push(cloneHistoryBlock(incoming))
}

class ProgressiveHistoryAccumulator {
  private readonly sessionId: string
  private readonly baseMetadata: SessionIndexEntry
  private readonly messages: ChatMessage[] = []
  private readonly toolCalls: ToolCall[] = []
  private readonly toolCallIndexById = new Map<string, number>()
  private sessionUsage: SessionUsage | undefined
  private plan: PlanEntry[] | undefined
  private planSeen = false
  private openRole: ChatMessage['role'] | null = null
  private baselineUsed: number | undefined
  private cursorValue = 0
  private targetLastSeqValue: number | undefined
  private loadedRecordCountValue = 0
  private pageNumberValue = 0

  constructor(metadata: SessionIndexEntry) {
    this.sessionId = metadata.id
    this.baseMetadata = { ...metadata, messageCount: 0, lastSeq: 0 }
  }

  get cursor(): number {
    return this.cursorValue
  }

  get targetLastSeq(): number | undefined {
    return this.targetLastSeqValue
  }

  get loadedRecordCount(): number {
    return this.loadedRecordCountValue
  }

  get pageNumber(): number {
    return this.pageNumberValue
  }

  applyPage(page: ConversationHistoryPageV1, limit: number): void {
    assertConversationHistoryPage(page, {
      sessionId: this.sessionId,
      afterSeq: this.cursorValue,
      limit,
      targetLastSeq: this.targetLastSeqValue
    })
    for (const record of page.records) {
      historyRecordApplications += 1
      this.applyRecord(record)
    }
    this.cursorValue = page.nextCursor
    this.targetLastSeqValue = page.targetLastSeq
    this.loadedRecordCountValue += page.records.length
    this.pageNumberValue += 1
    historyPageApplications += 1
  }

  snapshot(): SessionPayload {
    const messages = this.messages.map((message) => ({
      ...message,
      blocks: message.blocks.map(cloneHistoryBlock)
    }))
    const toolCalls = this.toolCalls.map((toolCall) => ({ ...toolCall }))
    historyTranscriptEntriesCopied += messages.length
    historyToolEntriesCopied += toolCalls.length
    historySnapshotsCreated += 1
    return {
      metadata: {
        ...this.baseMetadata,
        messageCount: messages.length,
        lastSeq: this.cursorValue
      },
      messages,
      ...(toolCalls.length > 0 ? { toolCalls } : {}),
      ...(this.sessionUsage
        ? {
            sessionUsage: {
              ...this.sessionUsage,
              ...(this.sessionUsage.cost ? { cost: { ...this.sessionUsage.cost } } : {})
            }
          }
        : {}),
      ...(this.planSeen ? { plan: (this.plan ?? []).map((entry) => ({ ...entry })) } : {})
    }
  }

  private applyRecord(record: ConversationHistoryRecordV1): void {
    const payload = asJsonObject(record.payload)
    switch (record.type) {
      case 'user_prompt':
        this.openRole = null
        this.pushUserPrompt(record, payload)
        break
      case 'message_chunk':
        this.pushMessageChunk(record, payload)
        break
      case 'tool_call':
        this.openRole = null
        this.upsertToolCall(record, payload)
        break
      case 'tool_call_update':
        this.updateToolCall(payload)
        break
      case 'prompt_complete':
        this.openRole = null
        break
      case 'usage_update':
        this.updateUsage(record, payload)
        break
      case 'plan_update':
        this.updatePlan(payload)
        break
      default:
        // Payload-free cursor markers are intentionally absent from records. Unknown future
        // renderer records consume their canonical cursor but never persist opaque payload data.
        break
    }
  }

  private pushUserPrompt(record: ConversationHistoryRecordV1, payload: JsonObject | null): void {
    const turnId = typeof payload?.turnId === 'string' && payload.turnId ? payload.turnId : null
    const content = Array.isArray(payload?.content)
      ? payload.content
          .map(asContentBlock)
          .filter((block): block is ContentBlock => block !== null)
          .map(cloneHistoryBlock)
      : []
    this.messages.push({
      id: turnId ? `turn:${turnId}` : `user:seq-${record.seq}`,
      role: 'user',
      blocks: content,
      streaming: false,
      timestamp: record.recordedAt,
      seq: record.seq
    })
  }

  private pushMessageChunk(record: ConversationHistoryRecordV1, payload: JsonObject | null): void {
    const role: ChatMessage['role'] = payload?.role === 'thought' ? 'thought' : 'agent'
    const content = asContentBlock(payload?.content)
    if (!content) return
    if (this.openRole === role) {
      const last = this.messages.at(-1)
      if (last) appendHistoryBlock(last.blocks, content)
      return
    }
    if (content.type === 'text' && !(content.text ?? '')) return
    this.openRole = role
    this.messages.push({
      id: `snapshot:${role}:${record.seq}`,
      role,
      blocks: [cloneHistoryBlock(content)],
      streaming: false,
      timestamp: record.recordedAt,
      seq: record.seq
    })
  }

  private upsertToolCall(record: ConversationHistoryRecordV1, payload: JsonObject | null): void {
    const toolCall = asJsonObject(payload?.toolCall)
    if (!toolCall || typeof toolCall.toolCallId !== 'string' || !toolCall.toolCallId) return
    const stamped: ToolCall = {
      ...(toolCall as ToolCall),
      timestamp: typeof toolCall.timestamp === 'number' ? toolCall.timestamp : record.recordedAt,
      seq: typeof toolCall.seq === 'number' ? toolCall.seq : record.seq
    }
    historyToolIndexLookups += 1
    const index = this.toolCallIndexById.get(stamped.toolCallId)
    if (index === undefined) {
      this.toolCallIndexById.set(stamped.toolCallId, this.toolCalls.length)
      this.toolCalls.push(stamped)
      return
    }
    const previous = this.toolCalls[index]
    this.toolCalls[index] = {
      ...previous,
      ...stamped,
      timestamp: previous.timestamp,
      seq: previous.seq
    }
  }

  private updateToolCall(payload: JsonObject | null): void {
    const update = asJsonObject(payload?.update)
    if (!update || typeof update.toolCallId !== 'string' || !update.toolCallId) return
    historyToolIndexLookups += 1
    const index = this.toolCallIndexById.get(update.toolCallId)
    if (index === undefined) return
    const previous = this.toolCalls[index]
    this.toolCalls[index] = {
      ...previous,
      ...(update as ToolCall),
      timestamp: previous.timestamp,
      seq: previous.seq
    }
  }

  private updateUsage(record: ConversationHistoryRecordV1, payload: JsonObject | null): void {
    const used = payload?.used
    const size = payload?.size
    if (
      typeof used !== 'number' ||
      typeof size !== 'number' ||
      !Number.isSafeInteger(used) ||
      !Number.isSafeInteger(size) ||
      used < 0 ||
      size <= 0
    ) {
      return
    }
    this.baselineUsed ??= used
    const cost = asJsonObject(payload?.cost)
    const validCost =
      typeof cost?.amount === 'number' &&
      Number.isFinite(cost.amount) &&
      cost.amount > 0 &&
      typeof cost.currency === 'string' &&
      cost.currency.length > 0
        ? { amount: cost.amount, currency: cost.currency }
        : undefined
    this.sessionUsage = {
      used,
      size,
      baselineUsed: this.baselineUsed,
      ...(validCost ? { cost: validCost } : {}),
      updatedAt: record.recordedAt,
      source: 'reported'
    }
  }

  private updatePlan(payload: JsonObject | null): void {
    const plan = asJsonObject(payload?.plan)
    if (!Array.isArray(plan?.entries)) return
    this.planSeen = true
    this.plan = plan.entries.flatMap((entry) => {
      const object = asJsonObject(entry)
      return object !== null && typeof object.content === 'string'
        ? [{ ...(object as PlanEntry) }]
        : []
    })
  }
}

interface PartialHistoryAssembly {
  mode: 'server' | 'tauri_store'
  accumulator: ProgressiveHistoryAccumulator
  publishedPayload?: SessionPayload
  publishedProgress?: HistoryPageProgress
  storedAt: number
  payloadBytes: number
}

export interface FailedPrefixResumeMetadata {
  sessionId: string
  cursor: number
  targetLastSeq: number
  errorCode: string
}

interface HistoryProgressSubscriber {
  options: LoadSessionPayloadOptions
  tail: Promise<void>
}

interface HistoryLoadFlight {
  mode: 'server' | 'tauri_store'
  subscribers: Set<HistoryProgressSubscriber>
  promise: Promise<SessionPayload | null>
  lastProgress?: HistoryPageProgress
  lastPublishedPayload?: SessionPayload
  lastPublishedProgress?: HistoryPageProgress
}

const partialHistoryAssemblies = new Map<string, PartialHistoryAssembly>()
const failedPrefixResumeMetadata = new Map<
  string,
  FailedPrefixResumeMetadata & { storedAt: number; bytes: number }
>()
const historyLoadFlights = new Map<string, HistoryLoadFlight>()
let currentHistoryInFlightBytes = 0
let peakHistoryInFlightBytes = 0

export interface HistoryPagingMetrics {
  currentBytes: number
  peakBytes: number
  traversalStarts: number
  pageRequests: number
  pageApplications: number
  recordApplications: number
  transcriptEntriesCopied: number
  toolEntriesCopied: number
  toolIndexLookups: number
  snapshotsCreated: number
}

export function historyPagingMetrics(): HistoryPagingMetrics {
  return {
    currentBytes: currentHistoryInFlightBytes,
    peakBytes: peakHistoryInFlightBytes,
    traversalStarts: historyTraversalStarts,
    pageRequests: historyPageRequests,
    pageApplications: historyPageApplications,
    recordApplications: historyRecordApplications,
    transcriptEntriesCopied: historyTranscriptEntriesCopied,
    toolEntriesCopied: historyToolEntriesCopied,
    toolIndexLookups: historyToolIndexLookups,
    snapshotsCreated: historySnapshotsCreated
  }
}

function encodedSessionPayloadBytes(payload: SessionPayload): number {
  try {
    return new TextEncoder().encode(JSON.stringify(payload)).length
  } catch {
    return 0
  }
}

function estimateAssemblyBytes(assembly: PartialHistoryAssembly): number {
  // snapshot() is a published-projection primitive, not a tape measure. Use the
  // already-published payload when present; otherwise report 0 without cloning.
  return assembly.publishedPayload ? encodedSessionPayloadBytes(assembly.publishedPayload) : 0
}

function evictFailedPrefixAssemblies(now = Date.now()): void {
  for (const [id, assembly] of [...partialHistoryAssemblies.entries()]) {
    if (now - assembly.storedAt > FAILED_PREFIX_TTL_MS) {
      partialHistoryAssemblies.delete(id)
    }
  }
  while (partialHistoryAssemblies.size > MAX_FAILED_PREFIX_ASSEMBLIES) {
    const oldest = partialHistoryAssemblies.keys().next().value
    if (oldest === undefined) break
    partialHistoryAssemblies.delete(oldest)
  }
  let totalBytes = 0
  for (const assembly of partialHistoryAssemblies.values()) {
    totalBytes += assembly.payloadBytes
  }
  if (totalBytes <= MAX_FAILED_PREFIX_PAYLOAD_BYTES) return
  for (const id of [...partialHistoryAssemblies.keys()]) {
    const assembly = partialHistoryAssemblies.get(id)
    if (!assembly) continue
    partialHistoryAssemblies.delete(id)
    totalBytes -= assembly.payloadBytes
    if (totalBytes <= MAX_FAILED_PREFIX_PAYLOAD_BYTES) return
  }
}

function evictResumeMetadata(now = Date.now()): void {
  for (const [id, entry] of [...failedPrefixResumeMetadata.entries()]) {
    if (now - entry.storedAt > RESUME_METADATA_TTL_MS) {
      failedPrefixResumeMetadata.delete(id)
    }
  }
  while (failedPrefixResumeMetadata.size > MAX_RESUME_METADATA_ENTRIES) {
    const oldest = failedPrefixResumeMetadata.keys().next().value
    if (oldest === undefined) break
    failedPrefixResumeMetadata.delete(oldest)
  }
  let totalBytes = 0
  for (const entry of failedPrefixResumeMetadata.values()) {
    totalBytes += entry.bytes
  }
  if (totalBytes <= RESUME_METADATA_MAX_BYTES) return
  for (const id of [...failedPrefixResumeMetadata.keys()]) {
    const entry = failedPrefixResumeMetadata.get(id)
    if (!entry) continue
    failedPrefixResumeMetadata.delete(id)
    totalBytes -= entry.bytes
    if (totalBytes <= RESUME_METADATA_MAX_BYTES) return
  }
}

function rememberFailedPrefixResume(
  id: string,
  assembly: PartialHistoryAssembly,
  errorCode: string,
  now = Date.now()
): void {
  const metadata: FailedPrefixResumeMetadata = {
    sessionId: id,
    cursor: assembly.accumulator.cursor,
    targetLastSeq: assembly.accumulator.targetLastSeq ?? 0,
    errorCode
  }
  const bytes = new TextEncoder().encode(JSON.stringify(metadata)).length
  failedPrefixResumeMetadata.delete(id)
  failedPrefixResumeMetadata.set(id, { ...metadata, storedAt: now, bytes })
  evictResumeMetadata(now)
}

function retainFailedPrefixAssembly(
  id: string,
  assembly: PartialHistoryAssembly,
  errorCode: string,
  now = Date.now()
): void {
  assembly.storedAt = now
  if (assembly.payloadBytes <= 0) {
    assembly.payloadBytes = estimateAssemblyBytes(assembly)
  }
  partialHistoryAssemblies.delete(id)
  partialHistoryAssemblies.set(id, assembly)
  rememberFailedPrefixResume(id, assembly, errorCode, now)
  evictFailedPrefixAssemblies(now)
}

export function clearFailedPrefixPayload(id: string): void {
  partialHistoryAssemblies.delete(id)
}

export function disposeFailedPrefixPayloads(): void {
  partialHistoryAssemblies.clear()
}

export function _failedPrefixIdsForTesting(): string[] {
  return [...partialHistoryAssemblies.keys()]
}

export function _resumeMetadataForTesting(id: string): FailedPrefixResumeMetadata | undefined {
  const entry = failedPrefixResumeMetadata.get(id)
  if (!entry) return undefined
  return {
    sessionId: entry.sessionId,
    cursor: entry.cursor,
    targetLastSeq: entry.targetLastSeq,
    errorCode: entry.errorCode
  }
}

export function _seedFailedPrefixForTesting(
  id: string,
  payloadBytes: number,
  storedAt = Date.now(),
  errorCode = 'TRANSPORT_ERROR'
): void {
  const metadata: SessionIndexEntry = {
    id,
    agentId: 'test',
    title: id,
    cwd: '/',
    projectId: '',
    createdAt: 0,
    lastActivityAt: 0,
    messageCount: 0,
    status: 'closed'
  }
  const assembly: PartialHistoryAssembly = {
    mode: 'tauri_store',
    accumulator: new ProgressiveHistoryAccumulator(metadata),
    storedAt,
    payloadBytes
  }
  retainFailedPrefixAssembly(id, assembly, errorCode, storedAt)
}

export function _resetHistoryPagingForTesting(): void {
  partialHistoryAssemblies.clear()
  failedPrefixResumeMetadata.clear()
  historyLoadFlights.clear()
  currentHistoryInFlightBytes = 0
  peakHistoryInFlightBytes = 0
  historyTraversalStarts = 0
  historyPageRequests = 0
  historyPageApplications = 0
  historyRecordApplications = 0
  historyTranscriptEntriesCopied = 0
  historyToolEntriesCopied = 0
  historyToolIndexLookups = 0
  historySnapshotsCreated = 0
}

function stablePayload(payload: SessionPayload): string {
  return JSON.stringify(payload)
}

export function toPersistedSessionSummaries(
  entries: SessionIndexEntry[]
): PersistedSessionSummary[] {
  return entries.map((entry) => ({
    storageKey: entry.conversationId ?? entry.id,
    sessionId: entry.id,
    stableAgentNamespace: entry.agentConfigId ? `config:${entry.agentConfigId}` : null,
    runtimeAgentId: entry.agentId || undefined,
    projectId: entry.projectId || undefined,
    cwd: entry.cwd,
    title: entry.title,
    createdAt: entry.createdAt,
    lastActivityAt: entry.lastActivityAt,
    status: entry.status === 'initializing' ? 'active' : entry.status,
    messageCount: entry.messageCount,
    discovered: entry.discovered,
    toolCount: 0,
    lastSeq: entry.lastSeq ?? 0,
    resumeEligible: Boolean(entry.agentConfigId || entry.agentId),
    worktreePath: entry.worktreePath,
    worktreeBranch: entry.worktreeBranch
  }))
}

export function deriveTitle(messages: ChatMessage[], fallbackTitle: string): string {
  const firstUser = messages.find((message) => message.role === 'user')
  if (firstUser) {
    const text = firstUser.blocks
      .map((block) => (block.type === 'text' ? (block.text ?? '') : ''))
      .join(' ')
      .trim()
    const firstLine = text.split(/\r?\n/, 1)[0].trim()
    if (firstLine.length > 0) {
      const characters = Array.from(firstLine)
      return characters.length > 48 ? `${characters.slice(0, 48).join('')}…` : firstLine
    }
  }
  return fallbackTitle
}

export type RecencyGroup = 'Today' | 'Yesterday' | 'Earlier'

export function groupSessionsByRecency<T extends { lastActivityAt: number }>(
  entries: T[],
  now: number
): { group: RecencyGroup; entries: T[] }[] {
  const startOfToday = new Date(now)
  startOfToday.setHours(0, 0, 0, 0)
  const todayMs = startOfToday.getTime()
  const yesterdayMs = todayMs - 24 * 60 * 60 * 1000
  const buckets: Record<RecencyGroup, T[]> = { Today: [], Yesterday: [], Earlier: [] }
  for (const entry of entries) {
    if (entry.lastActivityAt >= todayMs) buckets.Today.push(entry)
    else if (entry.lastActivityAt >= yesterdayMs) buckets.Yesterday.push(entry)
    else buckets.Earlier.push(entry)
  }
  return (['Today', 'Yesterday', 'Earlier'] as const)
    .map((group) => ({
      group,
      entries: buckets[group].slice().sort((a, b) => b.lastActivityAt - a.lastActivityAt)
    }))
    .filter(({ entries: grouped }) => grouped.length > 0)
}

export function scopeSessionIndex(
  entries: SessionIndexEntry[],
  projectId: string,
  cwd: string,
  worktreePaths: string[] = []
): SessionIndexEntry[] {
  if (!projectId || !cwd) return []
  // ADR 0002 scoping with worktree-inclusive reachability: in addition to an
  // exact-cwd match, a session whose cwd is one of the active project's
  // registered worktree paths stays listed. This keeps a worktree chat
  // reachable from the project root view and across restarts where
  // `activeWorktreeId` is null (the sidebar would otherwise hide it because
  // its cwd differs from the root). Falls back to projectId-only matching when
  // the scoped set is empty, preserving the prior drift-tolerant behavior.
  //
  // `normalizeCwdForScope` is required: the host persists session cwds via Rust
  // `Path::canonicalize`, which on Windows yields the verbatim `\\?\` prefix
  // with backslash separators (`\\?\E:\…\wt`), while the project store's
  // worktree paths come from `worktreeApi.list` in forward-slash form
  // (`E:/…/wt`). A raw `===`/`Set.has` never equates the two and would
  // silently hide worktree chats from the root view.
  const normalizedCwd = normalizeCwdForScope(cwd)
  const worktreePathSet =
    worktreePaths.length > 0 ? new Set(worktreePaths.map(normalizeCwdForScope)) : null
  const scoped = entries.filter((entry) => {
    if (entry.projectId !== projectId) return false
    const entryCwd = normalizeCwdForScope(entry.cwd)
    if (entryCwd === normalizedCwd) return true
    return worktreePathSet?.has(entryCwd) ?? false
  })
  return scoped.length > 0 ? scoped : entries.filter((entry) => entry.projectId === projectId)
}

// Canonicalize a cwd/path for comparison: strip the Windows verbatim `\\?\`
// prefix, collapse the extended UNC verbatim prefix `\\?\UNC\` to `//` so it
// matches standard UNC `\\server\share`, unify separators to `/`, and trim
// trailing slashes. Shared between `scopeSessionIndex` (session cwd vs project
// worktree paths) and the launcher's worktree-registration dedup (new worktree
// path vs already-stored paths), so a trailing-slash or verbatim-prefix form
// mismatch can't defeat either check. No lowercasing — preserves case-sensitive
// matching on POSIX where the prefix and backslashes never occur (the transform
// is a no-op there).
export function normalizeCwdForScope(p: string): string {
  if (!p) return p
  return (
    p
      // Extended UNC verbatim prefix `\\?\UNC\server\share` → `//server/share`:
      // collapse to `//` BEFORE the generic verbatim-prefix strip so extended and
      // standard UNC (`\\server\share`) normalize identically.
      .replace(/^\\\\\?\\UNC\\/i, '//')
      .replace(/^\\\\\?\\/, '')
      .replace(/\\/g, '/')
      .replace(/\/+$/, '')
  )
}

function historyMode(): 'server' | 'live_only' | 'tauri_store' | undefined {
  return getAcpTransport().historyMode?.()
}

export function fromPersistedSessionSummary(entry: PersistedSessionSummary): SessionIndexEntry {
  return {
    id: entry.sessionId,
    conversationId: isConversationId(entry.storageKey) ? entry.storageKey : undefined,
    agentId: entry.runtimeAgentId ?? '',
    agentConfigId: entry.stableAgentNamespace?.startsWith('config:')
      ? entry.stableAgentNamespace.slice('config:'.length)
      : undefined,
    title: entry.title ?? runtimeT('chat', 'store.untitledFallback', 'Untitled Chat'),
    cwd: entry.cwd,
    projectId: entry.projectId ?? '',
    createdAt: entry.createdAt,
    lastActivityAt: entry.lastActivityAt,
    messageCount: entry.messageCount,
    lastSeq: entry.lastSeq,
    status: entry.status,
    discovered:
      entry.discovered ??
      (entry.messageCount === 0 && entry.toolCount === 0 && entry.lastSeq === 0),
    worktreePath: entry.worktreePath,
    worktreeBranch: entry.worktreeBranch
  }
}

function normalizeDesktopIndexEntry(entry: SessionIndexEntry): SessionIndexEntry {
  const conversationId = conversationIdForIndexEntry(entry)
  return conversationId === entry.conversationId ? entry : { ...entry, conversationId }
}

export async function loadSessionIndex(): Promise<SessionIndexEntry[]> {
  const transport = getAcpTransport()
  const mode = transport.historyMode?.()
  if (mode === 'server' && transport.listPersistedSessions) {
    return (await transport.listPersistedSessions()).map(fromPersistedSessionSummary)
  }
  if (mode === 'live_only') return []
  return (await acpHistoryApi.list()).sessions.map(normalizeDesktopIndexEntry)
}

type PendingHistoryWaiter = {
  resolve: () => void
  reject: (error: unknown) => void
}

type PendingHistoryOperation =
  | { kind: 'save'; payload: SessionPayload; waiters: PendingHistoryWaiter[] }
  | { kind: 'delete'; waiters: PendingHistoryWaiter[] }

const pendingHistoryOperations = new Map<string, PendingHistoryOperation>()
const deletedSessionIds = new Set<string>()
let historyDrain: Promise<void> | null = null
let pendingGenericWrite: Promise<void> = Promise.resolve()
let pendingGenericWriteCount = 0
/**
 * Memoized in-flight backend `acp_history_flush` promise. `beforeunload` +
 * `pagehide` + `closeAppWithPersistenceFlush` can all fire on close,
 * previously triggering 3× concurrent backend flush calls (race + the
 * Windows `Access is denied` os-error-5 failure). Concurrent callers await
 * the SAME promise so exactly one `acpHistoryApi.flush()` reaches the
 * backend. Mirrors the `waitForPendingSessionIndexWrite` in-flight-promise
 * pattern already in this file.
 */
let pendingHistoryFlush: Promise<void> | null = null

async function drainHistoryOperations(): Promise<void> {
  while (pendingHistoryOperations.size > 0) {
    const next = pendingHistoryOperations.entries().next().value as
      | [string, PendingHistoryOperation]
      | undefined
    if (!next) break
    const [sessionId, operation] = next
    pendingHistoryOperations.delete(sessionId)
    try {
      if (operation.kind === 'save') {
        await saveSessionPayload(sessionId, operation.payload)
      } else {
        await deleteSessionPayload(sessionId)
        deletedSessionIds.delete(sessionId)
      }
      for (const waiter of operation.waiters) waiter.resolve()
    } catch (error) {
      console.error('[acp] failed to persist session history', error)
      if (operation.kind === 'delete') {
        for (const waiter of operation.waiters) waiter.reject(error)
      } else {
        for (const waiter of operation.waiters) waiter.resolve()
      }
    }
  }
}

function ensureHistoryDrain(): void {
  if (historyDrain) return
  historyDrain = drainHistoryOperations().finally(() => {
    historyDrain = null
    if (pendingHistoryOperations.size > 0) ensureHistoryDrain()
  })
}

/** Coalesce streaming writes so only the latest full payload per session is retained. */
export function queueSessionPayloadSave(id: string, payload: SessionPayload): Promise<void> {
  if (deletedSessionIds.has(id)) return Promise.resolve()
  return new Promise<void>((resolve, reject) => {
    const existing = pendingHistoryOperations.get(id)
    const waiter = { resolve, reject }
    const waiters = existing ? [...existing.waiters, waiter] : [waiter]
    pendingHistoryOperations.set(id, { kind: 'save', payload, waiters })
    ensureHistoryDrain()
  })
}

/** Delete is ordered with writes and supersedes every stale pending save for the session. */
export function queueSessionPayloadDelete(id: string): Promise<void> {
  deletedSessionIds.add(id)
  payloadCache.delete(id)
  pinnedPayloads.delete(id)
  partialHistoryAssemblies.delete(id)
  return new Promise<void>((resolve, reject) => {
    const existing = pendingHistoryOperations.get(id)
    const waiter = { resolve, reject }
    const waiters = existing ? [...existing.waiters, waiter] : [waiter]
    pendingHistoryOperations.set(id, { kind: 'delete', waiters })
    ensureHistoryDrain()
  })
}

/** Compatibility tracker for non-session test/legacy callers. */
export function trackPendingIndexWrite(write: () => Promise<void>): Promise<void> {
  pendingGenericWriteCount += 1
  const chained = pendingGenericWrite.then(async () => {
    try {
      await write()
    } catch (error) {
      console.error('[acp] failed to persist session history', error)
    } finally {
      pendingGenericWriteCount = Math.max(0, pendingGenericWriteCount - 1)
    }
  })
  pendingGenericWrite = chained
  return chained
}

export async function waitForPendingSessionIndexWrite(): Promise<void> {
  while (historyDrain || pendingHistoryOperations.size > 0 || pendingGenericWriteCount > 0) {
    await Promise.all([historyDrain ?? Promise.resolve(), pendingGenericWrite])
  }
}

export function _resetPendingIndexWriteTrackerForTesting(): void {
  pendingHistoryOperations.clear()
  deletedSessionIds.clear()
  historyDrain = null
  pendingGenericWrite = Promise.resolve()
  pendingGenericWriteCount = 0
  pendingHistoryFlush = null
}

export async function saveSessionIndex(_entries: SessionIndexEntry[]): Promise<void> {
  // Index ownership moved to Rust; every payload save atomically updates it.
}

const payloadCache = new Map<string, SessionPayload>()
const pinnedPayloads = new Set<string>()

function touchPayload(id: string, payload: SessionPayload): void {
  payloadCache.delete(id)
  payloadCache.set(id, payload)
  evictInactivePayloads()
}

function evictInactivePayloads(): void {
  let inactive = [...payloadCache.keys()].filter((id) => !pinnedPayloads.has(id)).length
  if (inactive <= INACTIVE_PAYLOAD_CACHE_BUDGET) return
  for (const id of payloadCache.keys()) {
    if (pinnedPayloads.has(id)) continue
    payloadCache.delete(id)
    inactive -= 1
    if (inactive <= INACTIVE_PAYLOAD_CACHE_BUDGET) return
  }
}

export function markSessionPayloadPinned(id: string): void {
  pinnedPayloads.add(id)
}

export function unpinSessionPayload(id: string): void {
  pinnedPayloads.delete(id)
  clearFailedPrefixPayload(id)
  evictInactivePayloads()
}

export function getCachedSessionPayload(id: string): SessionPayload | undefined {
  const payload = payloadCache.get(id)
  if (payload) touchPayload(id, payload)
  return payload
}

export function setCachedSessionPayload(id: string, payload: SessionPayload): void {
  touchPayload(id, payload)
}

function historyErrorCode(error: unknown): string | undefined {
  if (typeof error !== 'object' || error === null || !('code' in error)) return undefined
  const code = typeof error.code === 'string' ? error.code : undefined
  return code && /^[A-Z0-9_]+$|^[a-z][a-z_]*$/.test(code) && code.length <= 64 ? code : undefined
}

function safeHistoryLogSessionId(id: string): string {
  return /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(id) ? id : '[redacted]'
}

function isTransientHistoryError(error: unknown): boolean {
  if (isTransientAcpTransportError(error)) return true
  const code = historyErrorCode(error)
  if (code) {
    return ['NETWORK_ERROR', 'closed', 'timeout', 'agent_crashed'].includes(code)
  }
  // A raw rejected invoke/network Error has no stable application code. Preserve partial pages so
  // a later user retry resumes at the same cursor instead of blanking or refetching page one.
  return error instanceof Error
}

async function historyMetadata(
  id: string,
  mode: 'server' | 'tauri_store',
  options: LoadSessionPayloadOptions
): Promise<SessionIndexEntry | null> {
  if (options.metadata) {
    if (options.metadata.id !== id) {
      throw new ConversationHistoryLoadError(
        'VALIDATION_ERROR',
        'history metadata id does not match the requested session'
      )
    }
    return options.metadata
  }
  if (mode === 'server') {
    const transport = getAcpTransport()
    if (!transport.listPersistedSessions) {
      throw new AcpTransportError(
        'CONVERSATION_HISTORY_PAGING_REQUIRED',
        'server history metadata listing is unavailable'
      )
    }
    const summary = (await transport.listPersistedSessions()).find(
      (candidate) => candidate.sessionId === id
    )
    return summary ? fromPersistedSessionSummary(summary) : null
  }
  return (await acpHistoryApi.list()).sessions.find((candidate) => candidate.id === id) ?? null
}

async function requestHistoryPage(
  id: string,
  mode: 'server' | 'tauri_store',
  afterSeq: number,
  limit: number,
  targetLastSeq?: number
): Promise<ConversationHistoryPageV1> {
  assertConversationHistoryPageRequest(afterSeq, limit, targetLastSeq)
  historyPageRequests += 1
  if (mode === 'server') {
    const transport = getAcpTransport()
    if (!transport.getSessionPayloadPage) {
      throw new AcpTransportError(
        'CONVERSATION_HISTORY_PAGING_REQUIRED',
        'bounded server history is unavailable'
      )
    }
    // The WebSocket facade owns its per-session target map and forwards the first frontier on all
    // continuation frames. The shared loader still validates that returned frontier below.
    return transport.getSessionPayloadPage(id, afterSeq, limit)
  }
  return acpHistoryApi.getPage(id, afterSeq, limit, targetLastSeq)
}

function historyPageBytes(page: ConversationHistoryPageV1): number {
  try {
    return conversationHistoryPageEncodedBytes(page)
  } catch (error) {
    if (
      error instanceof ConversationHistoryPageValidationError &&
      /encoded limit/.test(error.message)
    ) {
      throw new ConversationHistoryLoadError(
        'CONVERSATION_HISTORY_IN_FLIGHT_LIMIT',
        'history page exceeds the bounded in-flight budget'
      )
    }
    throw error
  }
}

function historyFlightKey(mode: 'server' | 'tauri_store', id: string): string {
  return `${mode}\0${id}`
}

function enqueueHistorySubscriber(
  subscriber: HistoryProgressSubscriber,
  callback: () => void | Promise<void>
): Promise<void> {
  subscriber.tail = subscriber.tail.then(callback)
  return subscriber.tail
}

function addHistorySubscriber(
  flight: HistoryLoadFlight,
  options: LoadSessionPayloadOptions
): Promise<void> {
  const subscriber: HistoryProgressSubscriber = { options, tail: Promise.resolve() }
  flight.subscribers.add(subscriber)
  return enqueueHistorySubscriber(subscriber, async () => {
    if (flight.lastPublishedPayload && flight.lastPublishedProgress) {
      await options.onPage?.(flight.lastPublishedPayload, {
        ...flight.lastPublishedProgress,
        resumed: true,
        inFlightBytes: 0
      })
    }
    if (flight.lastProgress) {
      await options.onProgress?.({ ...flight.lastProgress, resumed: true, inFlightBytes: 0 })
    }
  })
}

async function publishHistorySnapshot(
  flight: HistoryLoadFlight,
  assembly: PartialHistoryAssembly,
  payload: SessionPayload,
  progress: HistoryPageProgress
): Promise<void> {
  assembly.publishedPayload = payload
  assembly.publishedProgress = progress
  assembly.payloadBytes = encodedSessionPayloadBytes(payload)
  flight.lastPublishedPayload = payload
  flight.lastPublishedProgress = progress
  await Promise.all(
    [...flight.subscribers].map((subscriber) =>
      enqueueHistorySubscriber(subscriber, () => subscriber.options.onPage?.(payload, progress))
    )
  )
}

async function publishHistoryProgress(
  flight: HistoryLoadFlight,
  progress: HistoryPageProgress
): Promise<void> {
  flight.lastProgress = progress
  await Promise.all(
    [...flight.subscribers].map((subscriber) =>
      enqueueHistorySubscriber(subscriber, () => subscriber.options.onProgress?.(progress))
    )
  )
}

function accumulatorProgress(
  id: string,
  assembly: PartialHistoryAssembly,
  pageRecordCount: number,
  inFlightBytes: number,
  resumed: boolean,
  complete = false
): HistoryPageProgress {
  return {
    sessionId: id,
    pageNumber: assembly.accumulator.pageNumber,
    pageRecordCount,
    loadedRecordCount: assembly.accumulator.loadedRecordCount,
    nextCursor: assembly.accumulator.cursor,
    targetLastSeq: assembly.accumulator.targetLastSeq ?? assembly.accumulator.cursor,
    complete,
    inFlightBytes,
    resumed
  }
}

async function runHistoryTraversal(
  id: string,
  mode: 'server' | 'tauri_store',
  options: LoadSessionPayloadOptions,
  flight: HistoryLoadFlight
): Promise<SessionPayload | null> {
  const existingPartial = partialHistoryAssemblies.get(id)
  if (existingPartial && existingPartial.mode !== mode) partialHistoryAssemblies.delete(id)
  let assembly = existingPartial?.mode === mode ? existingPartial : undefined
  if (!assembly) {
    const metadata = await historyMetadata(id, mode, options)
    if (!metadata) return null
    assembly = {
      mode,
      accumulator: new ProgressiveHistoryAccumulator(metadata),
      storedAt: Date.now(),
      payloadBytes: 0
    }
    partialHistoryAssemblies.set(id, assembly)
  } else if (assembly.accumulator.cursor > 0 && assembly.accumulator.targetLastSeq !== undefined) {
    const resumedProgress = accumulatorProgress(id, assembly, 0, 0, true)
    const resumedPayload = assembly.publishedPayload ?? assembly.accumulator.snapshot()
    await publishHistorySnapshot(flight, assembly, resumedPayload, resumedProgress)
    await publishHistoryProgress(flight, resumedProgress)
  }

  let lastPageBytes = 0
  try {
    while (true) {
      const afterSeq = assembly.accumulator.cursor
      const page = await requestHistoryPage(
        id,
        mode,
        afterSeq,
        RENDERER_HISTORY_PAGE_SIZE,
        assembly.accumulator.targetLastSeq
      )
      assertConversationHistoryPage(page, {
        sessionId: id,
        afterSeq,
        limit: RENDERER_HISTORY_PAGE_SIZE,
        targetLastSeq: assembly.accumulator.targetLastSeq
      })
      const inFlightBytes = historyPageBytes(page)
      lastPageBytes = inFlightBytes
      if (
        inFlightBytes > MAX_HISTORY_IN_FLIGHT_BYTES ||
        currentHistoryInFlightBytes + inFlightBytes > MAX_HISTORY_IN_FLIGHT_BYTES
      ) {
        throw new ConversationHistoryLoadError(
          'CONVERSATION_HISTORY_IN_FLIGHT_LIMIT',
          'history page exceeds the 4 MiB in-flight budget'
        )
      }

      currentHistoryInFlightBytes += inFlightBytes
      peakHistoryInFlightBytes = Math.max(peakHistoryInFlightBytes, currentHistoryInFlightBytes)
      try {
        assembly.accumulator.applyPage(page, RENDERER_HISTORY_PAGE_SIZE)
        const progress = accumulatorProgress(
          id,
          assembly,
          page.records.length,
          inFlightBytes,
          false,
          page.complete
        )
        let payload: SessionPayload | undefined
        if (assembly.accumulator.pageNumber === 1 || page.complete) {
          payload = assembly.accumulator.snapshot()
          await publishHistorySnapshot(flight, assembly, payload, progress)
        }
        await publishHistoryProgress(flight, progress)
        if (page.complete) {
          const completedPayload = payload ?? assembly.accumulator.snapshot()
          partialHistoryAssemblies.delete(id)
          touchPayload(id, completedPayload)
          return completedPayload
        }
      } finally {
        currentHistoryInFlightBytes = Math.max(0, currentHistoryInFlightBytes - inFlightBytes)
      }
    }
  } catch (error) {
    const transient = isTransientHistoryError(error)
    const code = historyErrorCode(error) ?? 'TRANSPORT_ERROR'
    if (assembly.accumulator.cursor > 0) {
      // Every later-page failure retains the verified prefix and pinned frontier. Stable failures
      // (for example `stale` or an oversized page) must not silently turn Retry history into a
      // cursor-zero traversal that can replace the visible prefix with a newer snapshot.
      const failureProgress = accumulatorProgress(id, assembly, 0, 0, true)
      if (assembly.publishedProgress?.nextCursor !== assembly.accumulator.cursor) {
        const retainedPayload = assembly.accumulator.snapshot()
        await publishHistorySnapshot(flight, assembly, retainedPayload, failureProgress)
      }
      flight.lastProgress = failureProgress
      retainFailedPrefixAssembly(id, assembly, code)
    } else {
      partialHistoryAssemblies.delete(id)
    }
    void logFrontendError({
      level: transient ? 'warn' : 'error',
      source: 'acp.historyPaging',
      message: `History page load failed sessionId=${safeHistoryLogSessionId(id)} code=${code} cursor=${assembly.accumulator.cursor} retainedRecordCount=${assembly.accumulator.loadedRecordCount} targetLastSeq=${assembly.accumulator.targetLastSeq ?? 0} pageBytes=${lastPageBytes}`
    })
    throw error
  }
}

export async function loadSessionPayload(
  id: string,
  options: LoadSessionPayloadOptions = {}
): Promise<SessionPayload | null> {
  if (options.metadata && options.metadata.id !== id) {
    throw new ConversationHistoryLoadError(
      'VALIDATION_ERROR',
      'history metadata id does not match the requested session'
    )
  }
  const transport = getAcpTransport()
  const negotiatedMode = transport.historyMode?.()
  if (negotiatedMode === 'live_only') return null
  const mode: 'server' | 'tauri_store' = negotiatedMode === 'server' ? 'server' : 'tauri_store'
  const key = historyFlightKey(mode, id)
  const existingFlight = historyLoadFlights.get(key)
  if (existingFlight) {
    await addHistorySubscriber(existingFlight, options)
    return existingFlight.promise
  }

  const partial = partialHistoryAssemblies.get(id)
  if (partial && partial.mode !== mode) partialHistoryAssemblies.delete(id)
  const currentPartial = partial?.mode === mode ? partial : undefined
  if (mode === 'tauri_store' && !currentPartial) {
    const cached = payloadCache.get(id)
    if (cached) {
      touchPayload(id, cached)
      const cursor = cached.metadata.lastSeq ?? maxPayloadSeq(cached)
      const progress: HistoryPageProgress = {
        sessionId: id,
        pageNumber: 0,
        pageRecordCount: 0,
        loadedRecordCount: cursor,
        nextCursor: cursor,
        targetLastSeq: cursor,
        complete: true,
        inFlightBytes: 0,
        resumed: true
      }
      await options.onPage?.(cached, progress)
      await options.onProgress?.(progress)
      return cached
    }
  }

  let resolveFlight!: (payload: SessionPayload | null) => void
  let rejectFlight!: (error: unknown) => void
  const promise = new Promise<SessionPayload | null>((resolve, reject) => {
    resolveFlight = resolve
    rejectFlight = reject
  })
  const flight: HistoryLoadFlight = {
    mode,
    subscribers: new Set(),
    promise
  }
  historyLoadFlights.set(key, flight)
  await addHistorySubscriber(flight, options)
  historyTraversalStarts += 1
  void runHistoryTraversal(id, mode, options, flight)
    .then(resolveFlight, rejectFlight)
    .finally(() => {
      if (historyLoadFlights.get(key) === flight) historyLoadFlights.delete(key)
    })
  return promise
}

export async function saveSessionPayload(id: string, payload: SessionPayload): Promise<void> {
  // CAP-2: the host event/session layer is now the sole author of durable
  // history in every mode (desktop shared-live included). Renderer payload
  // writes are retired; this stays a no-op so any residual queued save never
  // reaches a store. The payload cache still records the projection locally.
  if (deletedSessionIds.has(id)) return
  touchPayload(id, payload)
}

export async function deleteSessionPayload(id: string): Promise<void> {
  payloadCache.delete(id)
  pinnedPayloads.delete(id)
  partialHistoryAssemblies.delete(id)
  const mode = historyMode()
  if (mode === 'server' || mode === 'live_only') return
  await acpHistoryApi.delete(id)
}

export async function flushSessionHistory(): Promise<void> {
  const mode = historyMode()
  if (mode === 'server' || mode === 'live_only') return
  // Memoize the in-flight flush promise so `beforeunload` + `pagehide` +
  // `closeAppWithPersistenceFlush` (which can all fire on close) await the
  // SAME backend `acp_history_flush` call instead of racing 3× concurrent
  // flushes (the race produced the Windows `Access is denied` os-error-5
  // failure). A later caller attaches to the in-flight promise and resolves
  // with it; the promise is cleared on settle so a fresh flush after close
  // is not deduped against a stale one.
  if (pendingHistoryFlush) return pendingHistoryFlush
  const flushPromise = (async () => {
    await waitForPendingSessionIndexWrite()
    await acpHistoryApi.flush()
  })()
  pendingHistoryFlush = flushPromise
  try {
    await flushPromise
  } finally {
    // Clear so a subsequent close-path flush (e.g. a second window close
    // after the first settled) can invoke the backend again.
    if (pendingHistoryFlush === flushPromise) {
      pendingHistoryFlush = null
    }
  }
}

export function _clearPayloadCacheForTesting(): void {
  payloadCache.clear()
  pinnedPayloads.clear()
  _resetHistoryPagingForTesting()
}

export async function runHistoryWipeMigration(): Promise<void> {
  const mode = historyMode()
  if (mode === 'server' || mode === 'live_only') return

  // Reads/verification target the LEGACY store (read-your-writes). Host
  // convergence happens inside `save` / `markLegacyImportComplete`.
  const rustState = await acpHistoryApi.listLegacy()
  if (rustState.legacyImportComplete) return

  const indexResult = await persistenceApi.read<SessionIndexEntry[]>(SESSION_INDEX_KEY)
  if (!indexResult.success && indexResult.code !== 'KEY_NOT_FOUND') {
    throw new Error(indexResult.error)
  }
  if (indexResult.success && !Array.isArray(indexResult.data)) {
    throw new Error('Legacy session index is not an array')
  }
  const legacyIndex = indexResult.success ? indexResult.data : []
  const payloads: SessionPayload[] = []
  for (const entry of legacyIndex) {
    const payloadResult = await persistenceApi.read<SessionPayload>(sessionPayloadKey(entry.id))
    if (!payloadResult.success || !payloadResult.data) {
      throw new Error(payloadResult.success ? 'Legacy payload is empty' : payloadResult.error)
    }
    if (payloadResult.data.metadata.id !== entry.id) {
      throw new Error(`Legacy payload id mismatch for ${entry.id}`)
    }
    if (payloadResult.data.messages.length !== entry.messageCount) {
      throw new Error(`Legacy payload message count mismatch for ${entry.id}`)
    }
    payloads.push(payloadResult.data)
  }

  const legacyById = new Map(payloads.map((payload) => [payload.metadata.id, payload]))
  for (const entry of rustState.sessions) {
    const legacy = legacyById.get(entry.id)
    if (!legacy) continue
    const existing = await acpHistoryApi.getLegacy(entry.id)
    if (!existing || stablePayload(existing) !== stablePayload(legacy)) {
      throw new Error(`Durable history differs from legacy session ${entry.id}; import left intact`)
    }
  }
  for (const payload of payloads) {
    if (!rustState.sessions.some((entry) => entry.id === payload.metadata.id)) {
      await acpHistoryApi.save(payload.metadata.id, payload)
    }
  }

  const verified = await acpHistoryApi.listLegacy()
  for (const legacy of payloads) {
    const verifiedEntry = verified.sessions.find((entry) => entry.id === legacy.metadata.id)
    if (
      !verifiedEntry ||
      verifiedEntry.messageCount !== legacy.messages.length ||
      verifiedEntry.title !== legacy.metadata.title ||
      verifiedEntry.projectId !== legacy.metadata.projectId ||
      verifiedEntry.cwd !== legacy.metadata.cwd
    ) {
      throw new Error(`Legacy import metadata verification failed for ${legacy.metadata.id}`)
    }
    const durable = await acpHistoryApi.getLegacy(legacy.metadata.id)
    if (!durable || stablePayload(durable) !== stablePayload(legacy)) {
      throw new Error(`Legacy payload verification failed for ${legacy.metadata.id}`)
    }
  }

  try {
    for (const entry of legacyIndex) {
      const result = await persistenceApi.delete(sessionPayloadKey(entry.id))
      if (!result.success) throw new Error(result.error)
    }
    if (indexResult.success) {
      const result = await persistenceApi.delete(SESSION_INDEX_KEY)
      if (!result.success) throw new Error(result.error)
    }
    await acpHistoryApi.markLegacyImportComplete()
  } catch (error) {
    // Fail closed even if cleanup fails part-way: restore the complete legacy
    // source so the next launch can retry instead of inheriting a partial set.
    const rollbackFailures: string[] = []
    for (const payload of payloads) {
      const result = await persistenceApi.write(sessionPayloadKey(payload.metadata.id), payload)
      if (!result.success) rollbackFailures.push(result.error)
    }
    if (indexResult.success) {
      const result = await persistenceApi.write(SESSION_INDEX_KEY, legacyIndex)
      if (!result.success) rollbackFailures.push(result.error)
    }
    if (rollbackFailures.length > 0) {
      throw new Error(
        `${error instanceof Error ? error.message : String(error)}; rollback failed: ${rollbackFailures.join('; ')}`
      )
    }
    throw error
  }
}
