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
import { isTauriContext } from '@/lib/tauri-runtime'
import { webServerCliSessions } from '@/lib/web-server-api'
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
  const transcript = raw.transcript_path ?? raw.transcriptPath
  return {
    ...(raw.source === 'hook' || raw.source === 'poll' ? { source: raw.source } : {}),
    ...(isSafeAbsolutePath(transcript) ? { transcript_path: transcript } : {}),
    rootPid: raw.rootPid,
    agentId: raw.agentId,
    sessionId: raw.sessionId,
    selfDev: raw.selfDev === true,
    ...(isSafeAbsolutePath(raw.codexHome) ? { codexHome: raw.codexHome } : {}),
    cwd: isSafeAbsolutePath(raw.cwd) ? raw.cwd : null
  }
}

/** Desktop process discovery and remote host snapshot share one validated model. */
export async function detectLiveAgentSessions(rootPids: number[]): Promise<LiveAgentSession[]> {
  if (rootPids.length === 0) return []
  const result = isTauriContext()
    ? await invoke<unknown>('detect_live_agent_sessions_cmd', { rootPids })
    : await webServerCliSessions.live()
  const raw = Array.isArray(result)
    ? result
    : result && typeof result === 'object'
      ? (result as Record<string, unknown>).sessions
      : null
  if (!Array.isArray(raw)) return []
  const roots = new Set(rootPids)
  return raw
    .map(parseLiveAgentSession)
    .filter((s): s is LiveAgentSession => s !== null && roots.has(s.rootPid))
}

/**
 * The command that reopens a session. Each is run from the session's own
 * directory: claude and pi look sessions up per directory, codex would
 * otherwise stop to ask which directory to use, and qin-code would move the
 * session to wherever it was started.
 */
export function buildResumeArgv(session: LiveAgentSession): string[] {
  if (session.source === 'hook' && session.agentId === 'qin-code') return []
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
      return ['pi', '--session', session.transcript_path ?? id]
    case 'qin-code':
      return session.selfDev
        ? ['qin-code', 'self-dev', '--resume', id]
        : ['qin-code', '--resume', id]
  }
}

export function toTerminalAgentSession(session: LiveAgentSession): TerminalAgentSession {
  return {
    source: session.source ?? 'poll',
    ...(session.transcript_path ? { transcript_path: session.transcript_path } : {}),
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
    a.source === b.source &&
    a.descriptor_ref?.descriptor_id === b.descriptor_ref?.descriptor_id &&
    a.descriptor_ref?.generation === b.descriptor_ref?.generation &&
    a.transcript_path === b.transcript_path &&
    a.agentId === b.agentId &&
    a.sessionId === b.sessionId &&
    a.cwd === b.cwd &&
    a.resumeArgv.join('\0') === b.resumeArgv.join('\0')
  )
}

/**
 * Rehydrate durable identity while dropping legacy hook runtime. Host-backed
 * argv is display metadata; restore must resolve its descriptor before spawning.
 * Only legacy built-in metadata uses the historical safe argv reconstruction.
 */
export function reviveTerminalAgentSession(value: unknown): TerminalAgentSession | null {
  if (typeof value !== 'object' || value === null) return null
  const raw = value as Record<string, unknown>
  if (typeof raw.agentId !== 'string' || !/^[A-Za-z0-9_.-]{1,128}$/.test(raw.agentId)) return null
  if (typeof raw.sessionId !== 'string' || !SAFE_SESSION_ID.test(raw.sessionId)) return null
  const stored = Array.isArray(raw.resumeArgv) ? raw.resumeArgv : []
  const reference = raw.descriptor_ref as Record<string, unknown> | undefined
  const descriptor_ref =
    reference &&
    typeof reference.descriptor_id === 'string' &&
    reference.descriptor_id &&
    Number.isSafeInteger(reference.generation) &&
    (reference.generation as number) >= 0
      ? { descriptor_id: reference.descriptor_id, generation: reference.generation as number }
      : undefined
  const validStoredArgv =
    stored.length > 0 && stored.every((arg) => typeof arg === 'string' && !/[\r\n\0]/.test(arg))
  const identity = {
    source: (['hook', 'poll', 'process', 'legacy-unconfirmed'].includes(raw.source as string)
      ? raw.source
      : 'legacy-unconfirmed') as NonNullable<TerminalAgentSession['source']>,
    ...(descriptor_ref ? { descriptor_ref } : {}),
    ...(isSafeAbsolutePath(raw.transcript_path) ? { transcript_path: raw.transcript_path } : {}),
    ...(isSafeAbsolutePath(raw.cwd) ? { cwd: raw.cwd } : {})
  }
  // Unknown hook qin variant is metadata, not legacy executable authority.
  if (raw.source === 'hook' && raw.agentId === 'qin-code' && !descriptor_ref) {
    return { ...identity, agentId: raw.agentId, sessionId: raw.sessionId, resumeArgv: [] }
  }
  // Host-backed argv is display metadata only. Restore resolves descriptor_ref
  // again before executing. Legacy built-in entries keep their safe argv rebuild.
  if (descriptor_ref) {
    return {
      ...identity,
      agentId: raw.agentId,
      sessionId: raw.sessionId,
      resumeArgv: validStoredArgv ? ([...stored] as string[]) : []
    }
  }
  if (!isLiveAgentId(raw.agentId)) {
    if (!validStoredArgv) return null
    return {
      ...identity,
      agentId: raw.agentId,
      sessionId: raw.sessionId,
      resumeArgv: stored as string[]
    }
  }
  const codexHomeArg = stored.find(
    (arg): arg is string => typeof arg === 'string' && arg.startsWith('CODEX_HOME=')
  )
  const codexHome = codexHomeArg?.slice('CODEX_HOME='.length)
  return {
    ...toTerminalAgentSession({
      rootPid: 0,
      agentId: raw.agentId,
      sessionId: raw.sessionId,
      selfDev: stored.includes('self-dev'),
      ...identity,
      ...(isSafeAbsolutePath(codexHome) ? { codexHome } : {}),
      cwd: identity.cwd ?? null,
      source: 'poll'
    }),
    ...identity
  }
}

/** Hook qin without a host descriptor has no proven default/self-dev variant. */
export function needsManualAgentStart(session: TerminalAgentSession | null | undefined): boolean {
  return session?.source === 'hook' && session.agentId === 'qin-code' && !session.descriptor_ref
}

/** The line typed into a shell to reopen the session. */
export function resumeCommandLine(session: TerminalAgentSession): string {
  const [program, ...args] = session.resumeArgv
  return formatCliResumeCommand(program, args)
}
