import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { CachedTerminalSession } from './terminal-cache'
import {
  cacheTerminal,
  clearTerminalCache,
  disposeCachedTerminal,
  hasCachedTerminal,
  peekCachedTerminal,
  takeCachedTerminal
} from './terminal-cache'

function createSession(): CachedTerminalSession {
  return {
    terminal: {
      dispose: vi.fn(),
      element: { remove: vi.fn() }
    },
    fitAddon: {},
    searchAddon: {}
  } as unknown as CachedTerminalSession
}

describe('terminal-cache', () => {
  beforeEach(() => {
    clearTerminalCache()
  })

  it('preserves the terminal and persistent addons as one session', () => {
    const session = createSession()

    cacheTerminal('pty-1', session)

    expect(hasCachedTerminal('pty-1')).toBe(true)
    expect(takeCachedTerminal('pty-1')).toBe(session)
    expect(hasCachedTerminal('pty-1')).toBe(false)
    expect(session.terminal.dispose).not.toHaveBeenCalled()
  })

  it('hands back a cached session on peek without giving up ownership', () => {
    const session = createSession()
    cacheTerminal('pty-peek', session)

    expect(peekCachedTerminal('pty-peek')).toBe(session)
    // The detached-output sink peeks on every PTY chunk. If peeking removed the
    // session the way `take` does, the very first chunk would evict the terminal
    // the remount is about to reuse — and take its scrollback with it.
    expect(hasCachedTerminal('pty-peek')).toBe(true)
    expect(takeCachedTerminal('pty-peek')).toBe(session)
    expect(peekCachedTerminal('pty-peek')).toBeUndefined()
  })

  it('disposes a stale occupant before replacing the same PTY key', () => {
    const stale = createSession()
    const current = createSession()

    cacheTerminal('pty-1', stale)
    cacheTerminal('pty-1', current)

    expect(stale.terminal.dispose).toHaveBeenCalledTimes(1)
    expect(takeCachedTerminal('pty-1')).toBe(current)
  })

  it('disposes a cached renderer when its terminal is explicitly closed', () => {
    const session = createSession()
    cacheTerminal('pty-1', session)

    expect(disposeCachedTerminal('pty-1')).toBe(true)
    expect(session.terminal.dispose).toHaveBeenCalledTimes(1)
    expect(hasCachedTerminal('pty-1')).toBe(false)
    expect(disposeCachedTerminal('pty-1')).toBe(false)
  })

  it('evicts and disposes the oldest session when capacity is exceeded', () => {
    const sessions = Array.from({ length: 21 }, () => createSession())

    sessions.forEach((session, index) => {
      cacheTerminal(`pty-${index}`, session)
    })

    expect(hasCachedTerminal('pty-0')).toBe(false)
    expect(sessions[0].terminal.dispose).toHaveBeenCalledTimes(1)
    expect(hasCachedTerminal('pty-20')).toBe(true)
  })

  it('disposes every cached terminal when clearing the cache', () => {
    const first = createSession()
    const second = createSession()
    cacheTerminal('pty-1', first)
    cacheTerminal('pty-2', second)

    clearTerminalCache()

    expect(first.terminal.dispose).toHaveBeenCalledTimes(1)
    expect(second.terminal.dispose).toHaveBeenCalledTimes(1)
    expect(hasCachedTerminal('pty-1')).toBe(false)
    expect(hasCachedTerminal('pty-2')).toBe(false)
  })
})
