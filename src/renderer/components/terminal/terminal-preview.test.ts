import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Terminal } from '@/types/project'

const { mockGetTerminal, mockPeekCached, mockPeekTranscript } = vi.hoisted(() => ({
  mockGetTerminal: vi.fn(),
  mockPeekCached: vi.fn(),
  mockPeekTranscript: vi.fn()
}))

vi.mock('@/utils/terminal-registry', () => ({ getTerminal: mockGetTerminal }))
vi.mock('./terminal-cache', () => ({ peekCachedTerminal: mockPeekCached }))
vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: { getState: () => ({ peekTranscript: mockPeekTranscript }) }
}))

import { readTerminalPreview, screenLines } from './terminal-preview'

/** A stand-in xterm whose screen starts `baseY` rows into its buffer. */
function fakeXterm(buffer: string[], rows: number, baseY = 0) {
  return {
    cols: 80,
    rows,
    buffer: {
      active: {
        baseY,
        getLine: (index: number) =>
          index < buffer.length ? { translateToString: () => buffer[index] } : undefined
      }
    }
  }
}

const record: Terminal = { id: 't1', name: 'shell', shell: 'zsh', ptyId: 'pty-1' }

describe('screenLines', () => {
  it('reads the rows on screen and drops the blank rows under the output', () => {
    const xterm = fakeXterm(['old', 'prompt $ ls', 'a.txt', ''], 3, 1)
    expect(screenLines(xterm as never)).toEqual(['prompt $ ls', 'a.txt'])
  })
})

describe('readTerminalPreview', () => {
  beforeEach(() => {
    mockGetTerminal.mockReset()
    mockPeekCached.mockReset()
    mockPeekTranscript.mockReset()
    mockPeekTranscript.mockReturnValue('')
  })

  it('has nothing to show without a PTY', async () => {
    expect(await readTerminalPreview({ ...record, ptyId: undefined })).toBeNull()
  })

  it('reads a mounted terminal directly', async () => {
    mockGetTerminal.mockReturnValue(fakeXterm(['mounted'], 2))
    mockPeekTranscript.mockReturnValue('stale transcript')

    expect(await readTerminalPreview(record)).toEqual(['mounted'])
  })

  it('rebuilds the screen from detached output, redraws included', async () => {
    mockPeekCached.mockReturnValue({ terminal: fakeXterm(['before detach'], 4) })
    // A full-screen program clears and redraws; only the redrawn screen remains.
    mockPeekTranscript.mockReturnValue('scrolled away\r\n\u001b[2J\u001b[HAllow rspec?\r\n> Yes')

    expect(await readTerminalPreview(record)).toEqual(['Allow rspec?', '> Yes'])
  })

  it('falls back to the cached instance when nothing arrived since detach', async () => {
    mockPeekCached.mockReturnValue({ terminal: fakeXterm(['cached screen'], 4) })

    expect(await readTerminalPreview(record)).toEqual(['cached screen'])
  })
})
