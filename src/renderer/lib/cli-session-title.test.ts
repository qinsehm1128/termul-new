import type { DiscoveredCliSession } from '@shared/types/cli-session.types'
import { describe, expect, it } from 'vitest'
import { cliSessionListTitle } from '@/lib/cli-session-title'

function session(over: Partial<DiscoveredCliSession> = {}): DiscoveredCliSession {
  return {
    schemaVersion: 1,
    id: 'pi:abc:/tmp/a.jsonl',
    agentId: 'pi',
    sessionId: '',
    cwd: '/repo',
    title: '',
    createdAt: null,
    updatedAt: null,
    messageCount: 0,
    filePath: '/tmp/a.jsonl',
    resumable: false,
    ...over
  }
}

describe('cliSessionListTitle', () => {
  it('shows loading before hydrate', () => {
    expect(cliSessionListTitle(session(), 'loading', 'untitled')).toBe('loading')
  })

  it('prefers the first sentence over the session id', () => {
    expect(
      cliSessionListTitle(
        session({
          resumable: true,
          sessionId: '01a01802-155c-7f61-890e-3b203f207b0e',
          title: 'hi'
        }),
        'loading',
        'untitled'
      )
    ).toBe('hi')
  })

  it('does not display a raw session id as the title', () => {
    expect(
      cliSessionListTitle(
        session({
          resumable: true,
          sessionId: '01a01802-155c-7f61-890e-3b203f207b0e',
          title: '01a01802-155c-7f61-890e-3b203f207b0e'
        }),
        'loading',
        'untitled'
      )
    ).toBe('untitled')
  })
})
