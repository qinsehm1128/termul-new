import type { DiscoveredCliSession } from '@shared/types/cli-session.types'
import { describe, expect, it } from 'vitest'
import { getBuiltInAgent } from '@/lib/agents/agent-registry'
import {
  buildCliResumeArgv,
  formatCliResumeCommand,
  normalizeResumeFilePath,
  resumeHandleForSession
} from '@/lib/agents/cli-session-resume-argv'

function session(over: Partial<DiscoveredCliSession> = {}): DiscoveredCliSession {
  return {
    schemaVersion: 1,
    id: 'claude-code:abc:/tmp/a.jsonl',
    agentId: 'claude-code',
    sessionId: 'abc',
    cwd: '/repo',
    title: 'Hello',
    createdAt: null,
    updatedAt: null,
    messageCount: 1,
    filePath: '/tmp/a.jsonl',
    resumable: true,
    ...over
  }
}

describe('buildCliResumeArgv', () => {
  it('places extras before the resume flag and session id', () => {
    const def = getBuiltInAgent('claude-code')!
    const built = buildCliResumeArgv(def, session(), '--dangerously-skip-permissions', '--verbose')
    expect(built).toEqual({
      program: 'claude',
      args: ['--dangerously-skip-permissions', '--verbose', '--resume', 'abc']
    })
  })

  it('uses a subcommand for Codex', () => {
    const def = getBuiltInAgent('codex')!
    const built = buildCliResumeArgv(def, session({ agentId: 'codex', sessionId: 's1' }), '', '')
    expect(built).toEqual({ program: 'codex', args: ['resume', 's1'] })
  })

  it('uses --session and the session id for pi', () => {
    const def = getBuiltInAgent('pi')!
    const built = buildCliResumeArgv(
      def,
      session({
        agentId: 'pi',
        sessionId: '01a01876-6135-78d2-92e5-1523e69bf9e8',
        filePath: '/home/me/.pi/agent/sessions/a.jsonl',
        resumeFilePath: '/home/me/.pi/agent/sessions/a.jsonl'
      }),
      '',
      ''
    )
    expect(built).toEqual({
      program: 'pi',
      args: ['--session', '01a01876-6135-78d2-92e5-1523e69bf9e8']
    })
  })

  it('rejects a leading-dash session id', () => {
    const def = getBuiltInAgent('claude-code')!
    expect(buildCliResumeArgv(def, session({ sessionId: '-evil' }), '', '')).toEqual({
      error: 'Session is missing a safe resume handle'
    })
  })
})

describe('resume path guards', () => {
  it('quotes resume commands for a login shell', () => {
    expect(
      formatCliResumeCommand('pi', ['--session', '01a01876-6135-78d2-92e5-1523e69bf9e8'])
    ).toBe('pi --session 01a01876-6135-78d2-92e5-1523e69bf9e8')
    expect(formatCliResumeCommand('claude', ['--resume', "it's"])).toBe(
      "claude --resume 'it'\\''s'"
    )
  })

  it('rejects relative and parent paths', () => {
    expect(normalizeResumeFilePath('../x')).toBeNull()
    expect(normalizeResumeFilePath('/ok/../x')).toBeNull()
    expect(normalizeResumeFilePath('/ok/a.jsonl')).toBe('/ok/a.jsonl')
  })

  it('prefers session id for pi and falls back to the transcript path', () => {
    expect(
      resumeHandleForSession(
        session({
          agentId: 'pi',
          sessionId: '01a01876-6135-78d2-92e5-1523e69bf9e8',
          resumeFilePath: '/abs/session.jsonl',
          filePath: '/abs/session.jsonl'
        })
      )
    ).toBe('01a01876-6135-78d2-92e5-1523e69bf9e8')
    expect(
      resumeHandleForSession(
        session({
          agentId: 'pi',
          sessionId: '',
          resumeFilePath: '/abs/session.jsonl',
          filePath: '/abs/session.jsonl'
        })
      )
    ).toBe('/abs/session.jsonl')
  })
})
