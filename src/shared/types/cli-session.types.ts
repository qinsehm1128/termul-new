/**
 * Host-backed CLI agent session discovery (cwd-scoped resume).
 *
 * The host scans vendor transcript stores and returns metadata only. Resume
 * argv is assembled in the renderer and spawned through the existing PTY
 * path — the host never executes a resume command.
 */

export const CLI_SESSION_SCHEMA_VERSION = 1 as const

export const CLI_SESSION_AGENT_IDS = [
  'claude-code',
  'codex',
  'gemini-cli',
  'cursor',
  'opencode',
  'pi'
] as const

export type CliSessionAgentId = (typeof CLI_SESSION_AGENT_IDS)[number]

export const CLI_SESSION_SCOPE_PATHS_MAX = 64
export const CLI_SESSION_DEFAULT_LIMIT_PER_AGENT = 80
export const CLI_SESSION_ID_MAX_LENGTH = 512

export const CLI_SESSION_AGENT_LABELS: Record<CliSessionAgentId, string> = {
  'claude-code': 'Claude Code',
  codex: 'Codex',
  'gemini-cli': 'Gemini CLI',
  cursor: 'Cursor',
  opencode: 'OpenCode',
  pi: 'pi'
}

export interface DiscoveredCliSession {
  schemaVersion: typeof CLI_SESSION_SCHEMA_VERSION
  /** Stable list key: `{agentId}:{filePath}`. */
  id: string
  agentId: CliSessionAgentId
  /** Native resume handle from the first JSONL record. Empty until hydrated. */
  sessionId: string
  cwd: string | null
  title: string
  createdAt: string | null
  updatedAt: string | null
  messageCount: number
  filePath: string
  /** Codex home used when the session is not under the default ~/.codex. */
  codexHome?: string | null
  /** Absolute transcript path for agents that resume by file (pi). */
  resumeFilePath?: string | null
  resumable: boolean
}

export interface CliSessionScanIssue {
  agentId: CliSessionAgentId | 'unknown'
  path: string
  message: string
}

export interface CliSessionListArgs {
  scopePaths?: string[]
  agents?: CliSessionAgentId[]
  limit?: number
  force?: boolean
}

export interface CliSessionListResult {
  sessions: DiscoveredCliSession[]
  issues: CliSessionScanIssue[]
  scannedAt: string
}

export const CLI_SESSION_RESOLVE_BATCH_MAX = 16

export interface CliSessionResolveFile {
  agentId: CliSessionAgentId
  filePath: string
}

export interface CliSessionResolveArgs {
  files: CliSessionResolveFile[]
}

export interface CliSessionResolveResult {
  sessions: DiscoveredCliSession[]
  issues: CliSessionScanIssue[]
}

export interface CliResumeDefaultsV1 {
  schemaVersion: typeof CLI_SESSION_SCHEMA_VERSION
  extraArgsByAgentId: Partial<Record<CliSessionAgentId, string>>
}

export const DEFAULT_CLI_RESUME_EXTRA_ARGS: Record<CliSessionAgentId, string> = {
  'claude-code': '--dangerously-skip-permissions',
  codex: '--dangerously-bypass-approvals-and-sandbox',
  'gemini-cli': '--yolo',
  cursor: '--yolo',
  opencode: '',
  pi: ''
}

export function isCliSessionAgentId(value: unknown): value is CliSessionAgentId {
  return typeof value === 'string' && (CLI_SESSION_AGENT_IDS as readonly string[]).includes(value)
}

export function hasUnsafeCliSessionIdChars(value: string): boolean {
  for (let i = 0; i < value.length; i += 1) {
    const code = value.charCodeAt(i)
    if (code <= 0x1f || code === 0x7f) {
      return true
    }
  }
  return false
}

export function normalizeCliSessionId(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  if (
    trimmed.length === 0 ||
    trimmed.length > CLI_SESSION_ID_MAX_LENGTH ||
    trimmed.startsWith('-') ||
    hasUnsafeCliSessionIdChars(trimmed)
  ) {
    return null
  }
  return trimmed
}

export function parseCliResumeDefaults(value: unknown): CliResumeDefaultsV1 {
  const extraArgsByAgentId: Partial<Record<CliSessionAgentId, string>> = {
    ...DEFAULT_CLI_RESUME_EXTRA_ARGS
  }
  if (typeof value !== 'object' || value === null) {
    return { schemaVersion: CLI_SESSION_SCHEMA_VERSION, extraArgsByAgentId }
  }
  const record = value as Record<string, unknown>
  const raw = record.extraArgsByAgentId
  if (typeof raw === 'object' && raw !== null) {
    for (const [key, arg] of Object.entries(raw)) {
      if (isCliSessionAgentId(key) && typeof arg === 'string') {
        extraArgsByAgentId[key] = arg
      }
    }
  }
  return { schemaVersion: CLI_SESSION_SCHEMA_VERSION, extraArgsByAgentId }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

export function parseDiscoveredCliSession(value: unknown): DiscoveredCliSession | null {
  if (!isRecord(value)) return null
  if (value.schemaVersion !== CLI_SESSION_SCHEMA_VERSION) return null
  if (!isCliSessionAgentId(value.agentId)) return null
  if (typeof value.id !== 'string' || !value.id.trim()) return null
  if (typeof value.sessionId !== 'string') return null
  if (typeof value.resumable !== 'boolean') return null
  let sessionId = value.sessionId
  if (value.resumable) {
    const normalized = normalizeCliSessionId(sessionId)
    if (!normalized) return null
    sessionId = normalized
  } else if (sessionId && !normalizeCliSessionId(sessionId)) {
    return null
  }
  if (typeof value.title !== 'string') return null
  if (typeof value.filePath !== 'string' || !value.filePath.trim()) return null
  if (typeof value.messageCount !== 'number' || !Number.isFinite(value.messageCount)) {
    return null
  }
  if (value.cwd != null && typeof value.cwd !== 'string') return null
  if (value.createdAt != null && typeof value.createdAt !== 'string') return null
  if (value.updatedAt != null && typeof value.updatedAt !== 'string') return null
  if (value.codexHome != null && typeof value.codexHome !== 'string') return null
  if (value.resumeFilePath != null && typeof value.resumeFilePath !== 'string') return null
  return {
    schemaVersion: CLI_SESSION_SCHEMA_VERSION,
    id: value.id,
    agentId: value.agentId,
    sessionId,
    cwd: typeof value.cwd === 'string' ? value.cwd : null,
    title: value.title,
    createdAt: typeof value.createdAt === 'string' ? value.createdAt : null,
    updatedAt: typeof value.updatedAt === 'string' ? value.updatedAt : null,
    messageCount: value.messageCount,
    filePath: value.filePath,
    ...(typeof value.codexHome === 'string' ? { codexHome: value.codexHome } : {}),
    ...(typeof value.resumeFilePath === 'string' ? { resumeFilePath: value.resumeFilePath } : {}),
    resumable: value.resumable
  }
}

export function parseCliSessionListResult(value: unknown): CliSessionListResult | null {
  if (!isRecord(value)) return null
  if (!Array.isArray(value.sessions) || !Array.isArray(value.issues)) return null
  if (typeof value.scannedAt !== 'string') return null
  const sessions: DiscoveredCliSession[] = []
  for (const entry of value.sessions) {
    const parsed = parseDiscoveredCliSession(entry)
    if (!parsed) continue
    sessions.push(parsed)
  }
  const issues: CliSessionScanIssue[] = []
  for (const issue of value.issues) {
    if (!isRecord(issue)) return null
    if (issue.agentId !== 'unknown' && !isCliSessionAgentId(issue.agentId)) return null
    if (typeof issue.path !== 'string' || typeof issue.message !== 'string') return null
    issues.push({
      agentId: issue.agentId,
      path: issue.path,
      message: issue.message
    })
  }
  return { sessions, issues, scannedAt: value.scannedAt }
}

export function parseCliSessionResolveResult(value: unknown): CliSessionResolveResult | null {
  if (!isRecord(value)) return null
  if (!Array.isArray(value.sessions) || !Array.isArray(value.issues)) return null
  const sessions: DiscoveredCliSession[] = []
  for (const entry of value.sessions) {
    const parsed = parseDiscoveredCliSession(entry)
    if (!parsed) continue
    sessions.push(parsed)
  }
  const issues: CliSessionScanIssue[] = []
  for (const issue of value.issues) {
    if (!isRecord(issue)) return null
    if (issue.agentId !== 'unknown' && !isCliSessionAgentId(issue.agentId)) return null
    if (typeof issue.path !== 'string' || typeof issue.message !== 'string') return null
    issues.push({
      agentId: issue.agentId,
      path: issue.path,
      message: issue.message
    })
  }
  return { sessions, issues }
}

export interface CliSessionApi {
  listSessions: (args?: CliSessionListArgs) => Promise<CliSessionListResult>
  resolveSessions: (args: CliSessionResolveArgs) => Promise<CliSessionResolveResult>
}
