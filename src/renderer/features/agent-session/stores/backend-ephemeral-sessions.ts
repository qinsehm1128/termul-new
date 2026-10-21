import { type AgentId, acpApi, type SessionId } from '@/lib/acp-api'
import { brandedStorageKey } from '@/lib/brand-storage-key'

/**
 * Backend-ephemeral ACP sessions this renderer created and has not ended.
 *
 * Each one holds an agent-side process in the ACP Core (claude-agent-acp runs
 * one `claude` per session), and the Core outlives the renderer. A renderer
 * that reloaded or crashed forgot its sessions, so they ran until the Core
 * restarted — dozens of idle `claude` processes after a few days. The ids are
 * kept in localStorage so the next load can dispose what the previous one left.
 */
const STORAGE_KEY = brandedStorageKey(
  'acp.backendEphemeralSessions',
  'termul.acp.backendEphemeralSessions'
).canonical

export interface BackendEphemeralSession {
  agentId: AgentId
  sessionId: SessionId
}

function read(): Record<string, AgentId> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}')
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, AgentId>) : {}
  } catch {
    // Unreadable storage only means nothing to reclaim.
    return {}
  }
}

function write(sessions: Record<string, AgentId>): void {
  try {
    if (Object.keys(sessions).length === 0) {
      localStorage.removeItem(STORAGE_KEY)
    } else {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(sessions))
    }
  } catch {
    // Storage unavailable: the session is still ended normally, only a crash
    // before then leaves it to the Core's restart.
  }
}

export function rememberBackendEphemeralSession(agentId: AgentId, sessionId: SessionId): void {
  write({ ...read(), [sessionId]: agentId })
}

export function forgetBackendEphemeralSession(sessionId: SessionId): void {
  const sessions = read()
  if (!(sessionId in sessions)) return
  delete sessions[sessionId]
  write(sessions)
}

export function rememberedBackendEphemeralSessionIds(): SessionId[] {
  return Object.keys(read())
}

/** The sessions a previous load left behind; they are no longer remembered. */
export function takeBackendEphemeralSessions(): BackendEphemeralSession[] {
  const sessions = read()
  write({})
  return Object.entries(sessions).map(([sessionId, agentId]) => ({ agentId, sessionId }))
}

let previousLoadDisposed = false

/**
 * Dispose the sessions a previous load of this renderer left in the Core. The
 * backend only disposes a session still marked ephemeral, so one that was
 * already ended is refused rather than closed. Runs once per load: a second
 * run would take this load's own sessions.
 */
export async function disposeBackendEphemeralSessionsFromPreviousLoad(): Promise<void> {
  if (previousLoadDisposed) return
  previousLoadDisposed = true
  await Promise.all(
    takeBackendEphemeralSessions().map(({ agentId, sessionId }) =>
      acpApi.disposeEphemeralSession(agentId, sessionId).catch(() => {
        // Already ended, or its agent is gone: nothing is left to free.
      })
    )
  )
}

/** Test-only: let the next call dispose again, as a fresh load would. */
export function _resetPreviousLoadDisposalForTests(): void {
  previousLoadDisposed = false
}
