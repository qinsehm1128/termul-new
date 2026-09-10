/**
 * Cross-agent conversation memory index.
 *
 * The host scans past Claude Code / Codex / pi transcripts for one project,
 * normalizes them into one shape, and stores a searchable projection in a
 * host-private SQLite+FTS5 index. These are the wire contracts shared by the
 * Tauri commands, the HTTP routes and the two MCP surfaces.
 *
 * Three things worth knowing before consuming them:
 *
 * - `lineageDepth` is `0` root / `1` subagent / `2` a subagent's subagent, and
 *   `null` when the agent records no depth. `null` is not `0`: Codex's
 *   `agent_job` sessions carry neither a depth nor a parent, and rendering them
 *   as root messages would be a false claim, not a rounding.
 * - `firstMessageAtUtc` is the sort key, not `lastActivityAtUtc` and not the
 *   file's modification time.
 * - `sourceFresh: false` means the transcript changed after it was indexed, so
 *   the stored text may no longer be what that file says.
 */

export const MEMORY_INDEX_SCHEMA_VERSION = 1 as const

/** The agents v1 covers. Gemini, Cursor and OpenCode are deliberately absent. */
export const MEMORY_INDEX_AGENT_IDS = ['claude-code', 'codex', 'pi'] as const

export type MemoryIndexAgentId = (typeof MEMORY_INDEX_AGENT_IDS)[number]

export const MEMORY_INDEX_AGENT_LABELS: Record<MemoryIndexAgentId, string> = {
  'claude-code': 'Claude Code',
  codex: 'Codex',
  pi: 'pi'
}

export const MEMORY_INDEX_DEFAULT_LIMIT = 20
export const MEMORY_INDEX_MAX_LIMIT = 200

export type MemoryNormalizedRole =
  | 'user'
  | 'assistant'
  | 'tool_call'
  | 'tool_result'
  | 'system'
  | 'compaction'

export type MemoryTimestampConfidence = 'native' | 'session_start' | 'file_mtime' | 'unknown'

/** Whether a session's project ownership could be proven. */
export type MemorySessionScope = 'scoped' | 'unscoped'

/**
 * File identity plus the byte range of one record.
 *
 * `device` and `inode` are `0` on Windows, which has no such pair; the
 * remaining fields still carry the check there.
 */
export interface MemorySourcePointer {
  filePath: string
  device: number
  inode: number
  sizeBytes: number
  modifiedUnixMs: number
  /** SHA-256 of the pointed-to bytes, not of the whole file. */
  contentHash: string
  byteOffset: number
  byteLen: number
}

export interface MemoryIndexedSession {
  schemaVersion: number
  /** `{agentId}:{absolute transcript path}`. Stable list key. */
  sessionKey: string
  vendor: string
  vendorSessionId: string
  /** After flattening, this is what puts a subagent back with its conversation. */
  rootSessionKey: string
  /** `null` means the agent records no depth — never render it as `0`. */
  lineageDepth: number | null
  projectKey: string
  scope: MemorySessionScope
  cwd: string | null
  title: string | null
  /** The sort key. */
  firstMessageAtUtc: string | null
  firstMessageAtMs: number | null
  lastActivityAtUtc: string | null
  lastActivityAtMs: number | null
  timestampConfidence: MemoryTimestampConfidence
  messageCount: number
  toolCount: number
  filePath: string
  source: MemorySourcePointer
}

export interface MemorySearchHit {
  messageKey: string
  sessionKey: string
  rootSessionKey: string
  vendor: string
  lineageDepth: number | null
  ordinal: number
  role: MemoryNormalizedRole
  timestampUtc: string | null
  timestampMs: number | null
  timestampConfidence: MemoryTimestampConfidence
  toolName: string | null
  toolCallId: string | null
  /** Redacted, searchable text. Never the raw record. */
  text: string
  sessionTitle: string | null
  sessionFirstMessageAtUtc: string | null
  sessionScope: MemorySessionScope
  source: MemorySourcePointer
  /** `false` when the source transcript changed after indexing. */
  sourceFresh: boolean
}

/**
 * An agent-authored summary of context it was forced to drop, with a marker for
 * which entries were dropped. Only pi records these.
 */
export interface MemoryCompactionRecord {
  schemaVersion: number
  sessionKey: string
  rootSessionKey: string
  ordinal: number
  summary: string
  tokensBefore: number | null
  firstKeptEntryId: string | null
  timestampUtc: string | null
  timestampMs: number | null
  source: MemorySourcePointer
}

export interface MemorySearchRequest {
  query: string
  limit?: number | null
  /**
   * Restrict to particular agents. **Omit or leave empty to search every
   * agent**, which is the normal case — which CLI was running when something
   * was worked out is rarely what you remember about it.
   */
  agents?: MemoryIndexAgentId[]
  includeUnscoped?: boolean
  includeStale?: boolean
}

export interface MemorySearchResponse {
  projectKey: string
  query: string
  /** Returned ahead of ordinary hits. */
  compactions: MemoryCompactionRecord[]
  hits: MemorySearchHit[]
  /** Hits withheld because their source transcript changed since indexing. */
  staleHitsOmitted: number
}

export interface MemorySessionDetail {
  session: MemoryIndexedSession
  messages: MemorySearchHit[]
  staleMessagesOmitted: number
}

export interface MemoryIndexStatus {
  projectKey: string
  projectLabel: string
  databasePath: string
  exists: boolean
  sizeBytes: number
  sessionCount: number
  messageCount: number
  compactionCount: number
  newestFirstMessageAtUtc: string | null
}

export interface MemoryIndexIssue {
  code: string
  path: string
  detail: string
}

export interface MemoryIndexBuildReport {
  projectKey: string
  databasePath: string
  filesScanned: number
  sessionsIndexed: number
  sessionsSkippedUnchanged: number
  sessionsForgotten: number
  sessionsOutOfScope: number
  messagesIndexed: number
  compactionsIndexed: number
  durationMs: number
  issues: MemoryIndexIssue[]
}

export interface MemoryIndexBuildArgs {
  projectRoot: string
  fullRebuild?: boolean
  indexUnscoped?: boolean
}

export interface MemoryIndexScopeArgs {
  projectRoot: string
}

export interface MemoryIndexSearchArgs extends MemorySearchRequest {
  projectRoot: string
}

export interface MemoryIndexListArgs {
  projectRoot: string
  limit?: number | null
  includeUnscoped?: boolean
  agents?: MemoryIndexAgentId[]
}

export interface MemoryIndexSessionArgs {
  projectRoot: string
  sessionKey: string
  limit?: number | null
  includeStale?: boolean
}

/**
 * The capability, identical on desktop and web.
 *
 * `build` is the only mutating member, and the only one the UI calls from an
 * explicit user action — refreshing walks tens of thousands of files.
 */
export interface MemoryIndexApi {
  build(args: MemoryIndexBuildArgs): Promise<MemoryIndexBuildReport>
  status(args: MemoryIndexScopeArgs): Promise<MemoryIndexStatus>
  search(args: MemoryIndexSearchArgs): Promise<MemorySearchResponse>
  listSessions(args: MemoryIndexListArgs): Promise<MemoryIndexedSession[]>
  getSession(args: MemoryIndexSessionArgs): Promise<MemorySessionDetail | null>
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

/**
 * `lineageDepth` needs its own reader because `0` and `null` mean different
 * things and a `?? 0` anywhere in the chain would erase the distinction.
 */
export function readLineageDepth(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/** Human label for a depth, keeping "unknown" distinct from "root". */
export function lineageDepthLabel(depth: number | null): string {
  if (depth === null) return 'unknown'
  if (depth === 0) return 'root'
  if (depth === 1) return 'subagent'
  return `subagent (depth ${depth})`
}

export function parseMemoryIndexStatus(raw: unknown): MemoryIndexStatus | null {
  if (!isRecord(raw)) return null
  if (typeof raw.projectKey !== 'string' || typeof raw.exists !== 'boolean') return null
  return raw as unknown as MemoryIndexStatus
}

export function parseMemoryIndexBuildReport(raw: unknown): MemoryIndexBuildReport | null {
  if (!isRecord(raw)) return null
  if (typeof raw.projectKey !== 'string' || typeof raw.sessionsIndexed !== 'number') return null
  if (!Array.isArray(raw.issues)) return null
  return raw as unknown as MemoryIndexBuildReport
}

export function parseMemorySearchResponse(raw: unknown): MemorySearchResponse | null {
  if (!isRecord(raw)) return null
  if (!Array.isArray(raw.hits) || !Array.isArray(raw.compactions)) return null
  if (typeof raw.projectKey !== 'string') return null
  return raw as unknown as MemorySearchResponse
}

export function parseMemoryIndexedSessions(raw: unknown): MemoryIndexedSession[] | null {
  if (!Array.isArray(raw)) return null
  if (raw.some((entry) => !isRecord(entry) || typeof entry.sessionKey !== 'string')) return null
  return raw as unknown as MemoryIndexedSession[]
}

export function parseMemorySessionDetail(raw: unknown): MemorySessionDetail | null {
  if (raw === null || raw === undefined) return null
  if (!isRecord(raw) || !isRecord(raw.session) || !Array.isArray(raw.messages)) return null
  return raw as unknown as MemorySessionDetail
}
