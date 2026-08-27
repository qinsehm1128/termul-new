/**
 * Build argv for resuming a scanned CLI agent session.
 *
 * Order is always:
 *   `[...baseArgs, ...defaultExtra, ...onceExtra, ...resumeToken, handle]`
 *
 * Extra args never follow the session id. Resume opens a login shell at the
 * session cwd, then types this command into that shell.
 */
import type { CliSessionAgentId, DiscoveredCliSession } from '@shared/types/cli-session.types'
import { hasUnsafeCliSessionIdChars, normalizeCliSessionId } from '@shared/types/cli-session.types'

import type { AgentResumeMode, TerminalAgentDefinition } from './agent-registry'
import { parseBaseArgsInput } from './parse-base-args'

export function isUnsafeResumePath(value: string): boolean {
  if (!value.trim()) return true
  if (hasUnsafeCliSessionIdChars(value)) return true
  const parts = value.split(/[\\/]/)
  return parts.includes('..')
}

export function normalizeResumeFilePath(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  if (isUnsafeResumePath(trimmed)) return null
  const isPosixAbsolute = trimmed.startsWith('/')
  const isWindowsAbsolute = /^[A-Za-z]:[\\/]/.test(trimmed)
  if (!isPosixAbsolute && !isWindowsAbsolute) return null
  return trimmed
}

export function resumeHandleForSession(session: DiscoveredCliSession): string | null {
  const sessionId = normalizeCliSessionId(session.sessionId)
  if (sessionId) return sessionId
  if (session.agentId === 'pi') {
    return normalizeResumeFilePath(session.resumeFilePath ?? session.filePath)
  }
  return null
}

export function quoteShellArg(value: string): string {
  if (/^[A-Za-z0-9_./:=+-]+$/.test(value)) return value
  return `'${value.replace(/'/g, `'\\''`)}'`
}

export function formatCliResumeCommand(program: string, args: string[]): string {
  return [program, ...args.map(quoteShellArg)].join(' ')
}

export function buildCliResumeArgv(
  def: TerminalAgentDefinition,
  session: DiscoveredCliSession,
  defaultExtraArgs: string,
  onceExtraArgs: string
): { program: string; args: string[] } | { error: string } {
  if (!def.resumeMode) {
    return { error: `Agent ${def.id} does not support CLI session resume` }
  }
  if (!session.resumable) {
    return { error: 'Session is not resumable' }
  }
  const handle = resumeHandleForSession(session)
  if (!handle) {
    return { error: 'Session is missing a safe resume handle' }
  }

  const extras = [...parseBaseArgsInput(defaultExtraArgs), ...parseBaseArgsInput(onceExtraArgs)]

  return {
    program: def.command,
    args: [...def.baseArgs, ...extras, ...resumeTokens(def.resumeMode, handle)]
  }
}

function resumeTokens(mode: AgentResumeMode, handle: string): string[] {
  switch (mode.kind) {
    case 'flag':
    case 'file-flag':
      return [mode.token, handle]
    case 'subcommand':
      return [mode.token, handle]
  }
}

export function defaultExtraArgsForAgent(
  extraArgsByAgentId: Partial<Record<CliSessionAgentId, string>>,
  agentId: CliSessionAgentId
): string {
  return extraArgsByAgentId[agentId] ?? ''
}
