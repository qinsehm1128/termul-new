import { Terminal as XTerm } from '@xterm/xterm'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'
import { getTerminal } from '@/utils/terminal-registry'
import { peekCachedTerminal } from './terminal-cache'

/**
 * Enough recent output to rebuild a screen: full-screen programs redraw within
 * a few kilobytes, and older bytes only scroll past it.
 */
const PREVIEW_TAIL_CHARS = 64 * 1024
const FALLBACK_COLS = 120
const FALLBACK_ROWS = 30

interface ScreenSource {
  cols: number
  rows: number
  buffer: XTerm['buffer']
}

/** The rows on screen, bottom-aligned, without the blank rows below the output. */
export function screenLines(source: ScreenSource): string[] {
  const buffer = source.buffer.active
  const lines: string[] = []
  for (let row = 0; row < source.rows; row++) {
    lines.push(buffer.getLine(buffer.baseY + row)?.translateToString(true) ?? '')
  }
  while (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()
  return lines
}

function parseScreen(data: string, cols: number, rows: number): Promise<string[]> {
  // Never opened, so nothing is rendered: the parser alone rebuilds the screen.
  const parser = new XTerm({ cols, rows, scrollback: 0, allowProposedApi: true })
  return new Promise((resolve) => {
    parser.write(data, () => {
      const lines = screenLines(parser)
      parser.dispose()
      resolve(lines)
    })
  })
}

/**
 * What the terminal shows right now, read from what the renderer already
 * holds — no new stream and no instance kept around.
 *
 * A mounted terminal is current. Otherwise detached output goes either to the
 * transcript or into the cached instance (see use-terminal-detached-output), so
 * a non-empty transcript is the newer of the two and is replayed through a
 * throwaway parser at the cached instance's size.
 */
export async function readTerminalPreview(terminal: Terminal): Promise<string[] | null> {
  const ptyId = terminal.ptyId
  if (!ptyId) return null

  const mounted = getTerminal(ptyId)
  if (mounted) return screenLines(mounted)

  const cached = peekCachedTerminal(ptyId)?.terminal
  const transcript = useTerminalStore.getState().peekTranscript(ptyId)
  if (transcript) {
    return parseScreen(
      transcript.slice(-PREVIEW_TAIL_CHARS),
      cached?.cols ?? FALLBACK_COLS,
      cached?.rows ?? FALLBACK_ROWS
    )
  }
  return cached ? screenLines(cached) : null
}
