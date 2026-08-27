import { describe, expect, it } from 'vitest'
import {
  DEFAULT_CLI_RESUME_EXTRA_ARGS,
  normalizeCliSessionId,
  parseCliResumeDefaults,
  parseCliSessionListResult,
  parseDiscoveredCliSession
} from './cli-session.types'

describe('cli-session.types', () => {
  it('rejects session ids that are empty, leading-dash, or control-char', () => {
    expect(normalizeCliSessionId('')).toBeNull()
    expect(normalizeCliSessionId('   ')).toBeNull()
    expect(normalizeCliSessionId('-abc')).toBeNull()
    expect(normalizeCliSessionId('ab\nc')).toBeNull()
    expect(normalizeCliSessionId('ok-id_1')).toBe('ok-id_1')
  })

  it('merges resume defaults and keeps unknown agent keys out', () => {
    const parsed = parseCliResumeDefaults({
      schemaVersion: 1,
      extraArgsByAgentId: { 'claude-code': '--yolo', nope: '--x' }
    })
    expect(parsed.extraArgsByAgentId['claude-code']).toBe('--yolo')
    expect(parsed.extraArgsByAgentId.codex).toBe(DEFAULT_CLI_RESUME_EXTRA_ARGS.codex)
    expect(parsed.extraArgsByAgentId).not.toHaveProperty('nope')
  })

  it('parses a discovered session and rejects a bad schema', () => {
    const session = parseDiscoveredCliSession({
      schemaVersion: 1,
      id: 'claude-code:abc:/tmp/a.jsonl',
      agentId: 'claude-code',
      sessionId: 'abc',
      cwd: '/repo',
      title: 'Hello',
      createdAt: '2026-01-01T00:00:00.000Z',
      updatedAt: '2026-01-01T00:00:00.000Z',
      messageCount: 2,
      filePath: '/tmp/a.jsonl',
      resumable: true
    })
    expect(session?.sessionId).toBe('abc')
    expect(parseDiscoveredCliSession({ ...session, schemaVersion: 2 })).toBeNull()
  })

  it('parses a list result', () => {
    const result = parseCliSessionListResult({
      sessions: [],
      issues: [{ agentId: 'unknown', path: '/x', message: 'nope' }],
      scannedAt: '2026-01-01T00:00:00.000Z'
    })
    expect(result?.issues).toHaveLength(1)
    expect(parseCliSessionListResult({ sessions: [], issues: [] })).toBeNull()
  })

  it('allows an empty session id before lazy hydrate', () => {
    const session = parseDiscoveredCliSession({
      schemaVersion: 1,
      id: 'claude-code:/tmp/a.jsonl',
      agentId: 'claude-code',
      sessionId: '',
      cwd: '/repo',
      title: '',
      createdAt: null,
      updatedAt: null,
      messageCount: 0,
      filePath: '/tmp/a.jsonl',
      resumable: false
    })
    expect(session?.sessionId).toBe('')
    expect(session?.resumable).toBe(false)
  })

  it('skips invalid sessions instead of failing the list', () => {
    const result = parseCliSessionListResult({
      sessions: [
        { schemaVersion: 2 },
        {
          schemaVersion: 1,
          id: 'claude-code:abc:/tmp/a.jsonl',
          agentId: 'claude-code',
          sessionId: 'abc',
          cwd: '/repo',
          title: 'abc',
          createdAt: null,
          updatedAt: null,
          messageCount: 0,
          filePath: '/tmp/a.jsonl',
          resumable: true
        }
      ],
      issues: [],
      scannedAt: '2026-01-01T00:00:00.000Z'
    })
    expect(result?.sessions).toHaveLength(1)
    expect(result?.sessions[0]?.sessionId).toBe('abc')
  })
})
