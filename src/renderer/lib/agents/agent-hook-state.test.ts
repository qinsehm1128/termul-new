import type { AgentHookKind, DerivedState, HookStateDto } from '@shared/types/agent-hooks.types'
import { beforeEach, describe, expect, it } from 'vitest'
import { useAgentHookStateStore } from '@/stores/agent-hook-state-store'
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
beforeEach(() => useAgentHookStateStore.getState().reset())

function runtime(state: DerivedState, terminalId = 'pty'): HookStateDto {
  return {
    instance_id: 'host',
    terminal_id: terminalId,
    generation: 2,
    adapter_install_id: 'install',
    agent_session_id: 'safe-session',
    project_id: 'p',
    state,
    restore_state: 'confirmed',
    revision: 1,
    identity: {
      adapter_id: 'pi',
      agent_session_id: 'safe-session',
      transcript_path: null,
      cwd: null,
      source: 'hook',
      descriptor_id: 'descriptor',
      resume_argv: null
    }
  }
}
function publish(states: HookStateDto[]) {
  useAgentHookStateStore.getState().applySync({
    kind: 'snapshot',
    instance_id: 'host',
    current_revision: 1,
    states,
    tombstones: []
  })
}

describe('hook lifecycle contract', () => {
  it.each<[DerivedState, string | null]>([
    ['busy', 'working'],
    ['idle', 'idle'],
    ['needs_approval', 'blocked'],
    ['blocked', 'blocked'],
    ['error', 'blocked'],
    ['ended', null]
  ])('maps runtime %s to %s without persisting evidence', (state, expected) => {
    publish([runtime(state)])
    expect(hookTerminalState(hookEventSession(sample)!)).toBe(expected)
  })

  it('does not borrow ambiguous runtime state or a different descriptor generation', () => {
    const session = hookEventSession(sample)!
    publish([runtime('busy'), runtime('idle', 'other-pty')])
    expect(hookTerminalState(session)).toBeNull()
    useAgentHookStateStore.getState().reset()
    publish([runtime('busy')])
    expect(
      hookTerminalState({
        ...session,
        descriptor_ref: { descriptor_id: 'descriptor', generation: 1 }
      })
    ).toBeNull()
    expect(
      hookTerminalState({
        ...session,
        descriptor_ref: { descriptor_id: 'descriptor', generation: 2 }
      })
    ).toBe('working')
  })

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
  it('tracks provenance and transcript changes but ignores runtime changes in equality', () => {
    const session = hookEventSession(sample)!
    expect(isSameAgentSession(session, { ...session })).toBe(true)
    const withLegacyRuntime = {
      ...session,
      hook_kind: 'tool_start',
      hook_at: 900,
      tool_name: 'Read'
    }
    expect(isSameAgentSession(session, withLegacyRuntime)).toBe(true)
    expect(isSameAgentSession(session, { ...session, source: 'poll' })).toBe(false)
    expect(
      isSameAgentSession(session, { ...session, transcript_path: '/host/transcript.jsonl' })
    ).toBe(false)
    expect(session.resumeArgv).toContain('safe-session')
  })
})
