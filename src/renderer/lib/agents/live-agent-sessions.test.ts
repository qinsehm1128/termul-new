import type { HookStateDto } from '@shared/types/agent-hooks.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { hookStateSession } from './agent-hook-session'

const { mockInvoke, runtime, mockLive } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  runtime: { desktop: true },
  mockLive: vi.fn()
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => runtime.desktop }))
vi.mock('@/lib/web-server-api', () => ({ webServerCliSessions: { live: mockLive } }))

import {
  buildResumeArgv,
  detectLiveAgentSessions,
  isSameAgentSession,
  needsManualAgentStart,
  resumeCommandLine,
  reviveTerminalAgentSession,
  toTerminalAgentSession
} from './live-agent-sessions'

const base = { rootPid: 1, sessionId: 's-1', cwd: '/repo' }

describe('buildResumeArgv', () => {
  it('uses each agent’s own resume form', () => {
    expect(buildResumeArgv({ ...base, agentId: 'claude-code' })).toEqual([
      'claude',
      '--resume',
      's-1'
    ])
    expect(buildResumeArgv({ ...base, agentId: 'codex' })).toEqual(['codex', 'resume', 's-1'])
    expect(buildResumeArgv({ ...base, agentId: 'pi' })).toEqual(['pi', '--session', 's-1'])
    expect(buildResumeArgv({ ...base, agentId: 'qin-code' })).toEqual([
      'qin-code',
      '--resume',
      's-1'
    ])
  })

  it('keeps qin-code self-dev and a non-default CODEX_HOME', () => {
    expect(buildResumeArgv({ ...base, agentId: 'qin-code', selfDev: true })).toEqual([
      'qin-code',
      'self-dev',
      '--resume',
      's-1'
    ])
    expect(buildResumeArgv({ ...base, agentId: 'codex', codexHome: '/alt/codex' })).toEqual([
      'env',
      'CODEX_HOME=/alt/codex',
      'codex',
      'resume',
      's-1'
    ])
  })
})

describe('detectLiveAgentSessions', () => {
  beforeEach(() => {
    mockInvoke.mockReset()
    mockLive.mockReset()
    runtime.desktop = true
  })

  it('drops entries a shell could be tricked by', async () => {
    mockInvoke.mockResolvedValue([
      { rootPid: 10, agentId: 'claude-code', sessionId: 'ok-1', cwd: '/repo' },
      { rootPid: 11, agentId: 'claude-code', sessionId: 'x; rm -rf ~', cwd: '/repo' },
      { rootPid: 12, agentId: 'gemini-cli', sessionId: 'ok-2', cwd: '/repo' },
      { rootPid: 13, agentId: 'pi', sessionId: '-flag', cwd: '/repo' }
    ])

    const sessions = await detectLiveAgentSessions([10, 11, 12, 13])

    expect(mockInvoke).toHaveBeenCalledWith('detect_live_agent_sessions_cmd', {
      rootPids: [10, 11, 12, 13]
    })
    expect(sessions.map((s) => s.rootPid)).toEqual([10])
  })

  it.each([
    true,
    false
  ])('preserves identity but discards legacy hook runtime for desktop=%s', async (desktop) => {
    runtime.desktop = desktop
    const value = {
      ...base,
      agentId: 'pi',
      source: 'hook',
      hook_kind: 'needs_approval',
      transcript_path: '/host/session.jsonl'
    }
    mockInvoke.mockResolvedValue([value])
    mockLive.mockResolvedValue({ sessions: [value] })
    const [live] = await detectLiveAgentSessions([1])
    expect(live).toMatchObject({
      source: 'hook',
      transcript_path: '/host/session.jsonl'
    })
    expect(toTerminalAgentSession(live)).toMatchObject({
      source: 'hook',
      transcript_path: '/host/session.jsonl'
    })
    if (!desktop) expect(mockInvoke).not.toHaveBeenCalled()
  })
  it('drops old hook runtime, invalid provenance and unsafe transcript paths', async () => {
    mockInvoke.mockResolvedValue([
      {
        ...base,
        agentId: 'pi',
        source: 'hook',
        hookKind: 'tool_start',
        transcriptPath: '/host/log.jsonl'
      },
      {
        ...base,
        agentId: 'pi',
        source: 'anything',
        hook_kind: 'execute_shell',
        transcript_path: '/bad\npath'
      }
    ])
    const [valid, invalid] = await detectLiveAgentSessions([1])
    expect(valid).toMatchObject({
      source: 'hook',
      transcript_path: '/host/log.jsonl'
    })
    expect(invalid.source).toBeUndefined()
    expect(invalid).not.toHaveProperty('hook_kind')
    expect(valid).not.toHaveProperty('hook_kind')
    expect(invalid.transcript_path).toBeUndefined()
  })
  it('does not ask the host about no terminals', async () => {
    expect(await detectLiveAgentSessions([])).toEqual([])
    expect(mockInvoke).not.toHaveBeenCalled()
  })
})

describe('reviveTerminalAgentSession', () => {
  it('round-trips actual custom hook identity without optional display argv', () => {
    const state: HookStateDto = {
      instance_id: 'host',
      terminal_id: 'pty',
      generation: 0,
      adapter_install_id: 'custom-install',
      agent_session_id: 'custom-session',
      project_id: 'project',
      state: 'idle',
      restore_state: 'confirmed',
      revision: 1,
      identity: {
        adapter_id: 'custom-agent',
        agent_session_id: 'custom-session',
        descriptor_id: 'host-descriptor',
        source: 'hook',
        cwd: '/repo',
        transcript_path: null,
        resume_argv: null
      }
    }
    const session = hookStateSession(state)
    expect(session?.resumeArgv).toEqual([])
    expect(reviveTerminalAgentSession(JSON.parse(JSON.stringify(session)))).toEqual(session)
  })
  it.each(
    [[], ['qin-code', '--resume', 'unknown'], ['qin-code', 'self-dev', '--resume', 'unknown']].map(
      (resumeArgv) => ({ resumeArgv })
    )
  )('requires manual start for descriptorless hook qin regardless of legacy argv $resumeArgv', ({
    resumeArgv
  }) => {
    const restored = reviveTerminalAgentSession({
      agentId: 'qin-code',
      sessionId: 'unknown',
      source: 'hook',
      cwd: '/repo',
      resumeArgv
    })
    expect(restored?.resumeArgv).toEqual([])
    expect(needsManualAgentStart(restored ?? undefined)).toBe(true)
  })
  it('does not turn descriptorless live hook qin into default resume authority', () => {
    const live = toTerminalAgentSession({
      ...base,
      agentId: 'qin-code',
      source: 'hook',
      selfDev: false
    })
    expect(live.resumeArgv).toEqual([])
    expect(needsManualAgentStart(live)).toBe(true)
  })
  it('keeps legacy non-hook project snapshot default and self-dev revival', () => {
    for (const selfDev of [false, true]) {
      const restored = reviveTerminalAgentSession({
        agentId: 'qin-code',
        sessionId: 'legacy',
        resumeArgv: selfDev ? ['qin-code', 'self-dev', '--resume', 'legacy'] : []
      })
      expect(restored?.resumeArgv).toEqual([
        'qin-code',
        ...(selfDev ? ['self-dev'] : []),
        '--resume',
        'legacy'
      ])
      expect(needsManualAgentStart(restored ?? undefined)).toBe(false)
    }
  })
  it('rebuilds the argv from the validated fields', () => {
    const revived = reviveTerminalAgentSession({
      agentId: 'claude-code',
      sessionId: 'abc',
      cwd: '/repo',
      resumeArgv: ['rm', '-rf', '/']
    })
    expect(revived?.resumeArgv).toEqual(['claude', '--resume', 'abc'])
    expect(revived && resumeCommandLine(revived)).toBe('claude --resume abc')
  })

  it('keeps self-dev and CODEX_HOME that the stored argv carried', () => {
    expect(
      reviveTerminalAgentSession({
        agentId: 'qin-code',
        sessionId: 'session_x_1',
        resumeArgv: ['qin-code', 'self-dev', '--resume', 'session_x_1']
      })?.resumeArgv
    ).toEqual(['qin-code', 'self-dev', '--resume', 'session_x_1'])
    expect(
      reviveTerminalAgentSession({
        agentId: 'codex',
        sessionId: 'c1',
        resumeArgv: ['env', 'CODEX_HOME=/alt/codex', 'codex', 'resume', 'c1']
      })?.resumeArgv
    ).toEqual(['env', 'CODEX_HOME=/alt/codex', 'codex', 'resume', 'c1'])
  })

  it('rejects custom agents without display argv, unsafe ids and relative directories', () => {
    expect(reviveTerminalAgentSession({ agentId: 'gemini-cli', sessionId: 'a' })).toBeNull()
    expect(reviveTerminalAgentSession({ agentId: 'pi', sessionId: '$(boom)' })).toBeNull()
    expect(
      reviveTerminalAgentSession({ agentId: 'pi', sessionId: 'a', cwd: '../up' })?.cwd
    ).toBeUndefined()
    expect(reviveTerminalAgentSession(undefined)).toBeNull()
  })
})

describe('runtime-free durable identity', () => {
  it('pi always resumes the exact transcript rather than its session-id prefix', () => {
    const transcript_path = '/repo/.pi/sessions/2026-11-10T01-00-00_s-1.jsonl'
    expect(buildResumeArgv({ ...base, agentId: 'pi', transcript_path })).toEqual([
      'pi',
      '--session',
      transcript_path
    ])
    const identity = {
      agentId: 'pi',
      sessionId: 's-1',
      source: 'hook',
      descriptor_ref: { descriptor_id: 'durable', generation: 1 },
      transcript_path,
      cwd: '/repo',
      resumeArgv: ['pi', '--session', transcript_path]
    }
    expect(
      reviveTerminalAgentSession({
        ...identity,
        hook_kind: 'tool_start',
        hook_at: 123,
        tool_name: 'bash',
        state: 'busy',
        revision: 99
      })
    ).toEqual(identity)
  })
  it('keeps custom identity and host display argv without runtime or executable authority', () => {
    const identity = {
      agentId: 'custom-agent',
      sessionId: 's-1',
      source: 'process',
      descriptor_ref: { descriptor_id: 'custom-durable', generation: 1 },
      transcript_path: '/repo/transcript',
      cwd: '/repo',
      resumeArgv: ['custom-agent', 'restore', '--id', 's-1']
    }
    expect(
      reviveTerminalAgentSession({ ...identity, hook_kind: 'needs_approval', hook_at: 1 })
    ).toEqual(identity)
  })
  it('preserves host-backed builtin argv as display metadata and compares reference values', () => {
    const identity = {
      agentId: 'qin-code',
      sessionId: 's-1',
      source: 'hook' as const,
      descriptor_ref: { descriptor_id: 'durable', generation: 4 },
      cwd: '/repo',
      resumeArgv: ['qin-code', '--resume', 's-1', '--custom-display-flag']
    }
    expect(reviveTerminalAgentSession(identity)).toEqual(identity)
    expect(
      isSameAgentSession(identity, { ...identity, descriptor_ref: { ...identity.descriptor_ref } })
    ).toBe(true)
    expect(
      isSameAgentSession(identity, {
        ...identity,
        descriptor_ref: { descriptor_id: 'durable', generation: 5 }
      })
    ).toBe(false)
  })

  it('marks legacy provenance unconfirmed and ignores every runtime field in equality', () => {
    const identity = { agentId: 'codex', sessionId: 's-1', resumeArgv: ['codex', 'resume', 's-1'] }
    expect(reviveTerminalAgentSession(identity)?.source).toBe('legacy-unconfirmed')
    expect(
      isSameAgentSession(
        {
          ...identity,
          hook_kind: 'tool_start',
          hook_at: 1,
          tool_name: 'a',
          state: 'busy',
          revision: 1
        } as typeof identity,
        {
          ...identity,
          hook_kind: 'turn_done',
          hook_at: 999,
          tool_name: 'b',
          state: 'idle',
          revision: 100
        } as typeof identity
      )
    ).toBe(true)
    expect(
      isSameAgentSession(identity, {
        ...identity,
        descriptor_ref: { descriptor_id: 'new', generation: 1 }
      })
    ).toBe(false)
    expect(isSameAgentSession(identity, { ...identity, source: 'hook' })).toBe(false)
    expect(isSameAgentSession(identity, { ...identity, transcript_path: '/new' })).toBe(false)
  })
})

describe('isSameAgentSession', () => {
  it('compares what a restore would do', () => {
    const a = toTerminalAgentSession({ ...base, agentId: 'pi' })
    expect(isSameAgentSession(a, toTerminalAgentSession({ ...base, agentId: 'pi' }))).toBe(true)
    expect(
      isSameAgentSession(a, toTerminalAgentSession({ ...base, agentId: 'pi', sessionId: 's-2' }))
    ).toBe(false)
    expect(isSameAgentSession(a, undefined)).toBe(false)
    expect(isSameAgentSession(undefined, undefined)).toBe(true)
  })
})
