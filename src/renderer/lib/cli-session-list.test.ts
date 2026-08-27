import {
  CLI_SESSION_SCHEMA_VERSION,
  type DiscoveredCliSession
} from '@shared/types/cli-session.types'
import { describe, expect, it } from 'vitest'
import { filterCliSessions, groupCliSessions, sortCliSessions } from './cli-session-list'

function session(
  overrides: Partial<DiscoveredCliSession> & Pick<DiscoveredCliSession, 'id' | 'agentId'>
): DiscoveredCliSession {
  return {
    schemaVersion: CLI_SESSION_SCHEMA_VERSION,
    sessionId: overrides.id,
    cwd: '/work/termul',
    title: 'Session',
    createdAt: '2026-08-01T00:00:00.000Z',
    updatedAt: '2026-08-20T00:00:00.000Z',
    messageCount: 3,
    filePath: `/tmp/${overrides.id}.jsonl`,
    resumable: true,
    ...overrides
  }
}

describe('cli-session-list', () => {
  const older = session({
    id: 'a',
    agentId: 'claude-code',
    title: 'Older',
    updatedAt: '2026-08-10T00:00:00.000Z',
    createdAt: '2026-08-10T00:00:00.000Z'
  })
  const newer = session({
    id: 'b',
    agentId: 'codex',
    title: 'Newer',
    cwd: '/work/other',
    updatedAt: '2026-08-21T00:00:00.000Z',
    createdAt: '2026-07-01T00:00:00.000Z',
    messageCount: 0
  })

  it('sorts by updated then created', () => {
    expect(sortCliSessions([older, newer], 'updated').map((item) => item.id)).toEqual(['b', 'a'])
    expect(sortCliSessions([older, newer], 'created').map((item) => item.id)).toEqual(['a', 'b'])
  })

  it('filters empty sessions and search text', () => {
    expect(filterCliSessions([older, newer], '', true)).toEqual([older])
    expect(filterCliSessions([older, newer], 'other', false).map((item) => item.id)).toEqual(['b'])
    expect(
      filterCliSessions([older, newer], '', false, new Set(['claude-code'])).map((item) => item.id)
    ).toEqual(['a'])
  })

  it('groups by agent or folder basename', () => {
    const byAgent = groupCliSessions([older, newer], 'agent')
    expect(byAgent.map((group) => group.key)).toEqual(['claude-code', 'codex'])
    const byFolder = groupCliSessions([older, newer], 'folder')
    expect(byFolder.map((group) => group.label).sort()).toEqual(['other', 'termul'])
  })
})
