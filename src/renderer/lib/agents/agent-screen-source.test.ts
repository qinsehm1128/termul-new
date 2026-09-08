import type { Terminal } from '@xterm/xterm'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { readScreenTailFromTerminal, readTerminalScreenTail } from './agent-screen-source'

vi.mock('@/components/terminal/terminal-cache', () => ({
  peekCachedTerminal: vi.fn()
}))
vi.mock('@/utils/terminal-registry', () => ({
  getTerminal: vi.fn()
}))

const { peekCachedTerminal } = await import('@/components/terminal/terminal-cache')
const { getTerminal } = await import('@/utils/terminal-registry')

/**
 * Minimal xterm stand-in. `viewportY` is set away from the bottom on purpose:
 * anything that reads the viewport instead of the buffer tail will pick up the
 * wrong lines.
 */
function fakeTerminal(lines: string[], rows: number): Terminal {
  return {
    rows,
    buffer: {
      active: {
        length: lines.length,
        viewportY: 0,
        baseY: Math.max(0, lines.length - rows),
        getLine: (index: number) =>
          index >= 0 && index < lines.length ? { translateToString: () => lines[index] } : undefined
      }
    }
  } as unknown as Terminal
}

afterEach(() => {
  vi.mocked(getTerminal).mockReset()
  vi.mocked(peekCachedTerminal).mockReset()
})

describe('readScreenTailFromTerminal', () => {
  it('reads the last `rows` lines of the buffer', () => {
    const lines = Array.from({ length: 100 }, (_, i) => `line ${i}`)
    expect(readScreenTailFromTerminal(fakeTerminal(lines, 3))).toBe('line 97\nline 98\nline 99')
  })

  it('reads the bottom even when the user has scrolled to the top', () => {
    // viewportY is 0 in the fake, i.e. scrolled all the way back. The agent's
    // live prompt is still at the bottom, and that is what must be read.
    const lines = ['old prompt', 'middle', 'esc to interrupt']
    expect(readScreenTailFromTerminal(fakeTerminal(lines, 1))).toBe('esc to interrupt')
  })

  it('returns the whole buffer when it is shorter than the screen', () => {
    expect(readScreenTailFromTerminal(fakeTerminal(['a', 'b'], 24))).toBe('a\nb')
  })

  it('honours an explicit line count over the row count', () => {
    const lines = ['a', 'b', 'c', 'd']
    expect(readScreenTailFromTerminal(fakeTerminal(lines, 24), 2)).toBe('c\nd')
  })

  it('falls back to a fixed window when the terminal reports no rows', () => {
    const lines = Array.from({ length: 60 }, (_, i) => `line ${i}`)
    const tail = readScreenTailFromTerminal(fakeTerminal(lines, 0))
    expect(tail.split('\n')).toHaveLength(40)
    expect(tail.endsWith('line 59')).toBe(true)
  })

  it('substitutes an empty string for a line the buffer cannot produce', () => {
    const terminal = fakeTerminal(['a', 'b'], 2)
    vi.spyOn(terminal.buffer.active, 'getLine').mockReturnValue(undefined)
    expect(readScreenTailFromTerminal(terminal)).toBe('\n')
  })

  it('returns an empty string for an empty buffer', () => {
    expect(readScreenTailFromTerminal(fakeTerminal([], 24))).toBe('')
  })
})

describe('readTerminalScreenTail', () => {
  it('prefers the mounted instance', () => {
    vi.mocked(getTerminal).mockReturnValue(fakeTerminal(['mounted'], 1))
    vi.mocked(peekCachedTerminal).mockReturnValue({
      terminal: fakeTerminal(['cached'], 1)
    } as never)

    expect(readTerminalScreenTail('term-1', 'pty-1')).toBe('mounted')
    expect(peekCachedTerminal).not.toHaveBeenCalled()
  })

  it('falls back to the cached instance for a detached terminal', () => {
    vi.mocked(getTerminal).mockReturnValue(undefined)
    vi.mocked(peekCachedTerminal).mockReturnValue({
      terminal: fakeTerminal(['esc to interrupt'], 1)
    } as never)

    expect(readTerminalScreenTail('term-1', 'pty-1')).toBe('esc to interrupt')
    expect(peekCachedTerminal).toHaveBeenCalledWith('pty-1')
  })

  it('returns null when nothing holds a live xterm', () => {
    vi.mocked(getTerminal).mockReturnValue(undefined)
    vi.mocked(peekCachedTerminal).mockReturnValue(undefined)

    // Null, not '': a terminal with no screen is unknown, never idle.
    expect(readTerminalScreenTail('term-1', 'pty-1')).toBeNull()
  })

  it('does not consult the cache without a pty id', () => {
    vi.mocked(getTerminal).mockReturnValue(undefined)

    expect(readTerminalScreenTail('term-1', null)).toBeNull()
    expect(peekCachedTerminal).not.toHaveBeenCalled()
  })
})
