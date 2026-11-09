import type { AgentHookKind } from '@shared/types/agent-hooks.types'
import { describe, expect, it } from 'vitest'
import { hookEventSession } from './agent-hook-session'
import { hookKindState, hookTerminalState } from './agent-hook-state'
import { isSameAgentSession } from './live-agent-sessions'

const sample = {
  sequence: 1,
  terminal_id: 'pty',
  project_id: 'p',
  agent: 'pi' as const,
  session_id: 'safe-session',
  at: 1,
  kind: 'idle' as const
}
describe('hook lifecycle contract', () => {
  it.each<[AgentHookKind, string | null]>([
    ['session_start', 'idle'],
    ['session_end', null],
    ['prompt_submitted', 'working'],
    ['tool_start', 'working'],
    ['tool_end', 'working'],
    ['needs_approval', 'blocked'],
    ['blocked', 'blocked'],
    ['idle', 'idle'],
    ['turn_done', 'idle'],
    ['error', 'blocked']
  ])('maps %s to %s', (kind, expected) => expect(hookKindState(kind)).toBe(expected))
  it('ignores stale hook metadata on a poll-derived session', () => {
    const session = hookEventSession(sample)!
    expect(hookTerminalState({ ...session, source: 'poll' })).toBeNull()
    expect(hookTerminalState(undefined)).toBeNull()
  })
  it('tracks provenance, hook activity and transcript changes in session equality', () => {
    const session = hookEventSession(sample)!
    expect(isSameAgentSession(session, { ...session })).toBe(true)
    expect(isSameAgentSession(session, { ...session, hook_kind: 'tool_start' })).toBe(false)
    expect(isSameAgentSession(session, { ...session, source: 'poll' })).toBe(false)
    expect(
      isSameAgentSession(session, { ...session, transcript_path: '/host/transcript.jsonl' })
    ).toBe(false)
    expect(session.resumeArgv).toContain('safe-session')
  })
})
