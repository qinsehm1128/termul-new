/**
 * Agent sessions running inside terminals, and how to reopen them.
 *
 * The host finds which claude / codex / pi / qin-code session runs under each
 * terminal's shell (metadata only); this module turns that into the argv that
 * reopens the session, and re-validates a remembered one before it is typed
 * into a fresh shell.
 */
import {
  hasUnsafeCliSessionIdChars,
  LIVE_AGENT_IDS,
  type LiveAgentId,
  type LiveAgentSession,
  type TerminalAgentSession
} from '@shared/types/cli-session.types'
import { invoke } from '@tauri-apps/api/core'
import { formatCliResumeCommand } from './cli-session-resume-argv'

/** Ids are typed into a shell, so only plain id characters pass. */
const SAFE_SESSION_ID = /^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,511}$/

function isLiveAgentId(value: unknown): value is LiveAgentId {
  return typeof value === 'string' && (LIVE_AGENT_IDS as readonly string[]).includes(value)
}

function isSafeAbsolutePath(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    value.startsWith('/') &&
    !hasUnsafeCliSessionIdChars(value) &&
    !value.split('/').includes('..')
  )
}

function parseLiveAgentSession(value: unknown): LiveAgentSession | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  if (typeof raw.rootPid !== 'number' || !isLiveAgentId(raw.agentId)) return null
  if (typeof raw.sessionId !== 'string' || !SAFE_SESSION_ID.test(raw.sessionId)) return null
  return {
    rootPid: raw.rootPid,
    agentId: raw.agentId,
    sessionId: raw.sessionId,
    selfDev: raw.selfDev === true,
    ...(isSafeAbsolutePath(raw.codexHome) ? { codexHome: raw.codexHome } : {}),
    cwd: isSafeAbsolutePath(raw.cwd) ? raw.cwd : null
  }
}

/** Desktop only: the host reads this machine's process table. */
export async function detectLiveAgentSessions(rootPids: number[]): Promise<LiveAgentSession[]> {
  if (rootPids.length === 0) return []
  const raw = await invoke<unknown>('detect_live_agent_sessions_cmd', { rootPids })
  if (!Array.isArray(raw)) return []
  return raw.map(parseLiveAgentSession).filter((s): s is LiveAgentSession => s !== null)
}

/**
 * The command that reopens a session. Each is run from the session's own
 * directory: claude and pi look sessions up per directory, codex would
 * otherwise stop to ask which directory to use, and qin-code would move the
 * session to wherever it was started.
 */
export function buildResumeArgv(session: LiveAgentSession): string[] {
  const id = session.sessionId
  switch (session.agentId) {
    case 'claude-code':
      return ['claude', '--resume', id]
    case 'codex':
      // `codex resume` only searches its own home; a non-default one must be
      // the same home the session was written to.
      return session.codexHome
        ? ['env', `CODEX_HOME=${session.codexHome}`, 'codex', 'resume', id]
        : ['codex', 'resume', id]
    case 'pi':
      return ['pi', '--session', id]
    case 'qin-code':
      return session.selfDev
        ? ['qin-code', 'self-dev', '--resume', id]
        : ['qin-code', '--resume', id]
  }
}

export function toTerminalAgentSession(session: LiveAgentSession): TerminalAgentSession {
  return {
    agentId: session.agentId,
    sessionId: session.sessionId,
    ...(session.cwd ? { cwd: session.cwd } : {}),
    resumeArgv: buildResumeArgv(session)
  }
}

export function isSameAgentSession(
  a: TerminalAgentSession | undefined,
  b: TerminalAgentSession | undefined
): boolean {
  if (!a || !b) return a === b
  return (
    a.agentId === b.agentId &&
    a.sessionId === b.sessionId &&
    a.cwd === b.cwd &&
    a.resumeArgv.join('\0') === b.resumeArgv.join('\0')
  )
}

/**
 * A remembered session, checked again before use: it comes back from disk,
 * and its argv is about to be typed into a shell. The argv is rebuilt from the
 * validated fields rather than trusted as stored.
 */
export function reviveTerminalAgentSession(value: unknown): TerminalAgentSession | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  if (!isLiveAgentId(raw.agentId)) return null
  if (typeof raw.sessionId !== 'string' || !SAFE_SESSION_ID.test(raw.sessionId)) return null
  const stored = Array.isArray(raw.resumeArgv) ? raw.resumeArgv : []
  const codexHomeArg = stored.find(
    (arg): arg is string => typeof arg === 'string' && arg.startsWith('CODEX_HOME=')
  )
  const codexHome = codexHomeArg?.slice('CODEX_HOME='.length)
  return toTerminalAgentSession({
    rootPid: 0,
    agentId: raw.agentId,
    sessionId: raw.sessionId,
    selfDev: stored.includes('self-dev'),
    ...(isSafeAbsolutePath(codexHome) ? { codexHome } : {}),
    cwd: isSafeAbsolutePath(raw.cwd) ? raw.cwd : null
  })
}

/** The line typed into a shell to reopen the session. */
export function resumeCommandLine(session: TerminalAgentSession): string {
  const [program, ...args] = session.resumeArgv
  return formatCliResumeCommand(program, args)
}
