import {
  type HostContextPublishRequest,
  type HostContextState,
  parseHostContextPublishRequest,
  parseHostContextState
} from '@shared/types/plugin.types'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createHostContextPublisher, diffHostContext, projectHostContext } from './host-context'

const empty = (): HostContextState =>
  projectHostContext({
    projects: [],
    activeProjectId: '',
    terminals: [],
    sessions: [],
    activeTerminalId: null,
    activeSessionId: null,
    focus: { terminal: false, textInput: false }
  })
const ack = { success: true as const, data: { contextRevision: 1, sourceSequence: 1 } }
const sourceId = '123e4567-e89b-42d3-a456-426614174000'
afterEach(() => vi.useRealTimers())

describe('host context projection privacy boundary', () => {
  it('whitelists source fields and emits real session states without content', () => {
    const snapshot = projectHostContext({
      projects: [{ id: 'p', name: 'Project', path: '/private/project' }],
      activeProjectId: 'p',
      terminals: [{ id: 't', ptyId: 'pty', projectId: 'p', output: 'secret' } as never],
      sessions: [
        {
          id: 's',
          projectId: 'p',
          status: 'active',
          activeTurn: false,
          openTurnId: null,
          replaying: { text: 'secret' },
          messages: ['secret']
        } as never
      ],
      activeTerminalId: 't',
      activeSessionId: 's',
      focus: { terminal: true, textInput: false }
    })
    expect(parseHostContextState(snapshot)).toEqual(snapshot)
    expect(JSON.stringify(snapshot)).not.toContain('secret')
    expect(snapshot.agentSessions[0]).toEqual({
      id: 's',
      projectId: 'p',
      status: 'active',
      activeTurn: false,
      openTurnId: null,
      replaying: true
    })
  })

  it('caps arrays and UTF-8 snapshots, preserving active IDs when possible', () => {
    const snapshot = projectHostContext({
      projects: [{ id: 'p', name: '🚀'.repeat(300), path: '/'.repeat(6000) }],
      activeProjectId: 'p',
      terminals: Array.from({ length: 300 }, (_, i) => ({
        id: `t${i}`,
        projectId: 'p',
        ptyId: '🚀'.repeat(128)
      })),
      sessions: [],
      activeTerminalId: 't299',
      activeSessionId: null,
      focus: { terminal: false, textInput: false }
    })
    expect(snapshot.truncated).toBe(true)
    expect(snapshot.terminals.length).toBeLessThanOrEqual(256)
    expect(snapshot.activeTerminalId).toBe('t299')
    expect(Array.from(snapshot.activeProject!.name)).toHaveLength(128)
    expect(new TextEncoder().encode(JSON.stringify(snapshot)).length).toBeLessThanOrEqual(
      128 * 1024
    )
    expect(() => parseHostContextState(snapshot)).not.toThrow()
  })

  it('keeps the active session and terminal under byte pressure', () => {
    const snapshot = projectHostContext({
      projects: [{ id: 'p', name: 'P', path: '/p' }],
      activeProjectId: 'p',
      terminals: Array.from({ length: 256 }, (_, i) => ({
        id: `t${i}`,
        ptyId: '🚀'.repeat(128),
        projectId: 'p'
      })),
      sessions: [
        {
          id: 's',
          projectId: 'p',
          status: 'active',
          activeTurn: true,
          openTurnId: 'turn',
          replaying: false
        }
      ],
      activeTerminalId: 't255',
      activeSessionId: 's',
      focus: { terminal: false, textInput: false }
    })
    expect(snapshot.activeSessionId).toBe('s')
    expect(snapshot.activeTerminalId).toBe('t255')
    expect(snapshot.agentSessions[0]?.id).toBe('s')
    expect(new TextEncoder().encode(JSON.stringify(snapshot)).length).toBeLessThanOrEqual(
      128 * 1024
    )
    expect(() => parseHostContextState(snapshot)).not.toThrow()
  })

  it('retains the active project ID under project truncation', () => {
    const snapshot = projectHostContext({
      projects: Array.from({ length: 300 }, (_, i) => ({ id: `p${i}`, name: 'P', path: '/p' })),
      activeProjectId: 'p299',
      terminals: [],
      sessions: [],
      activeTerminalId: null,
      activeSessionId: null,
      focus: { terminal: false, textInput: false }
    })
    expect(snapshot.projectIds).toContain('p299')
    expect(snapshot.projectIds).toHaveLength(256)
    expect(snapshot.truncated).toBe(true)
  })

  it('does not close terminals when tab focus changes; closes only on registry removal', () => {
    const before = {
      ...empty(),
      activeTerminalId: 't',
      terminals: [{ id: 't', ptyId: 'pty', projectId: 'p' }]
    }
    const hidden = { ...before, activeTerminalId: null }
    expect(diffHostContext(before, hidden).map((e) => e.topic)).toEqual(['terminal.activeChanged'])
    expect(diffHostContext(hidden, { ...hidden, terminals: [] }).map((e) => e.topic)).toEqual([
      'terminal.closed'
    ])
  })

  it('captures all four session transition dimensions, including an immediate turn end', () => {
    const session = {
      id: 's',
      projectId: 'p',
      status: 'active' as const,
      activeTurn: false,
      openTurnId: null,
      replaying: false
    }
    let before = { ...empty(), agentSessions: [session] }
    for (const patch of [
      { activeTurn: true },
      { openTurnId: 'turn' },
      { replaying: true },
      { status: 'error' as const },
      { activeTurn: false }
    ]) {
      const after = { ...before, agentSessions: [{ ...before.agentSessions[0], ...patch }] }
      expect(diffHostContext(before, after)[0]?.topic).toBe('agent.session.stateChanged')
      before = after
    }
  })
})

describe('host context publisher', () => {
  it('captures synchronous transitions but throttles flushes and allows only one publication in flight', async () => {
    vi.useFakeTimers()
    let resolve!: (value: typeof ack) => void
    const publish = vi
      .fn()
      .mockResolvedValue(ack)
      .mockImplementationOnce(
        () =>
          new Promise((r) => {
            resolve = r
          })
      )
    const publisher = createHostContextPublisher({
      api: { publishHostContext: publish, heartbeatHostContext: vi.fn().mockResolvedValue(ack) },
      sourceId,
      initial: empty()
    })
    publisher.reconnect()
    await vi.advanceTimersByTimeAsync(0)
    publisher.capture({ ...empty(), activeProject: { id: 'p', name: 'P', path: '/p' } })
    publisher.capture(empty())
    await vi.advanceTimersByTimeAsync(100)
    expect(publish).toHaveBeenCalledTimes(1)
    resolve(ack)
    await vi.advanceTimersByTimeAsync(0)
    expect(publish).toHaveBeenCalledTimes(2)
    const second = publish.mock.calls[1][0] as HostContextPublishRequest
    expect(second.events.map((e) => e.topic)).toEqual([
      'project.activeChanged',
      'project.activeChanged'
    ])
    expect(second.reset).toBe(false)
    expect(() => parseHostContextPublishRequest(second)).not.toThrow()
    publisher.dispose()
  })

  it('resets overflowed transitions and reconnect, heartbeats at 2 seconds and stops on teardown', async () => {
    vi.useFakeTimers()
    const publish = vi.fn().mockResolvedValue(ack)
    const heartbeat = vi.fn().mockResolvedValue(ack)
    const publisher = createHostContextPublisher({
      api: { publishHostContext: publish, heartbeatHostContext: heartbeat },
      sourceId,
      initial: empty()
    })
    publisher.reconnect()
    await vi.advanceTimersByTimeAsync(0)
    for (let n = 0; n < 130; n++)
      publisher.capture({ ...empty(), activeProject: { id: `p${n}`, name: 'P', path: '/p' } })
    await vi.advanceTimersByTimeAsync(50)
    expect(publish.mock.calls[1][0].reset).toBe(true)
    expect(publish.mock.calls[1][0].events).toHaveLength(0)
    await vi.advanceTimersByTimeAsync(1950)
    expect(heartbeat).toHaveBeenCalledTimes(1)
    publisher.disconnect()
    await vi.advanceTimersByTimeAsync(2000)
    expect(heartbeat).toHaveBeenCalledTimes(1)
    publisher.reconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(publish.mock.calls[2][0].reset).toBe(true)
    publisher.dispose()
    await vi.advanceTimersByTimeAsync(5000)
    expect(heartbeat).toHaveBeenCalledTimes(1)
  })

  it('resets when crossing a truncated projection boundary instead of fabricating closures', async () => {
    vi.useFakeTimers()
    const publish = vi.fn().mockResolvedValue(ack)
    const publisher = createHostContextPublisher({
      api: { publishHostContext: publish, heartbeatHostContext: vi.fn().mockResolvedValue(ack) },
      sourceId,
      initial: {
        ...empty(),
        truncated: true,
        terminals: [{ id: 't', projectId: null, ptyId: null }]
      }
    })
    publisher.reconnect()
    await vi.advanceTimersByTimeAsync(0)
    publisher.capture(empty())
    await vi.advanceTimersByTimeAsync(50)
    expect(publish.mock.calls[1][0].reset).toBe(true)
    expect(publish.mock.calls[1][0].events).toEqual([])
    publisher.dispose()
  })

  it('marks context unavailable after heartbeat failure until reset publication', async () => {
    vi.useFakeTimers()
    const unavailable = vi.fn()
    const publish = vi.fn().mockResolvedValue(ack)
    const heartbeat = vi
      .fn()
      .mockResolvedValue({ success: false, code: 'PLUGIN_CONTEXT_UNAVAILABLE', error: 'expired' })
    const publisher = createHostContextPublisher({
      api: { publishHostContext: publish, heartbeatHostContext: heartbeat },
      sourceId,
      initial: empty(),
      onUnavailable: unavailable
    })
    publisher.reconnect()
    await vi.advanceTimersByTimeAsync(2000)
    expect(unavailable).toHaveBeenCalledTimes(1)
    expect(publish.mock.calls.at(-1)?.[0].reset).toBe(true)
    publisher.dispose()
  })

  it('requests reset after hydration/restore and transport errors', async () => {
    vi.useFakeTimers()
    const publish = vi.fn().mockRejectedValueOnce(new Error('offline')).mockResolvedValue(ack)
    const publisher = createHostContextPublisher({
      api: { publishHostContext: publish, heartbeatHostContext: vi.fn().mockResolvedValue(ack) },
      sourceId,
      initial: empty()
    })
    publisher.reconnect()
    await vi.advanceTimersByTimeAsync(50)
    expect(publish.mock.calls[1][0].reset).toBe(true)
    publisher.capture(empty(), true)
    await vi.advanceTimersByTimeAsync(50)
    expect(publish.mock.calls[2][0].reset).toBe(true)
    publisher.dispose()
  })
})
