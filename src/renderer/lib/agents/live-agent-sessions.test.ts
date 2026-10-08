import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }))

import {
  buildResumeArgv,
  detectLiveAgentSessions,
  isSameAgentSession,
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
  beforeEach(() => mockInvoke.mockReset())

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

  it('does not ask the host about no terminals', async () => {
    expect(await detectLiveAgentSessions([])).toEqual([])
    expect(mockInvoke).not.toHaveBeenCalled()
  })
})

describe('reviveTerminalAgentSession', () => {
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

  it('rejects unknown agents, unsafe ids and relative directories', () => {
    expect(reviveTerminalAgentSession({ agentId: 'gemini-cli', sessionId: 'a' })).toBeNull()
    expect(reviveTerminalAgentSession({ agentId: 'pi', sessionId: '$(boom)' })).toBeNull()
    expect(
      reviveTerminalAgentSession({ agentId: 'pi', sessionId: 'a', cwd: '../up' })?.cwd
    ).toBeUndefined()
    expect(reviveTerminalAgentSession(undefined)).toBeNull()
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
