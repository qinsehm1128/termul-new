/**
 * Reads the bottom of a terminal's screen for agent state detection.
 *
 * Two properties matter and both are easy to get wrong:
 *
 * - **Bottom of buffer, not the viewport.** The user can scroll a terminal
 *   anywhere; the agent's live prompt is always at the bottom. Indexing from
 *   `buffer.length` rather than `buffer.viewportY` keeps a scrolled-back user
 *   from changing what the indicator reports.
 * - **Detached terminals still count.** An xterm unregisters at unmount but
 *   stays alive as the sink for detached PTY output, so a backgrounded agent
 *   still has a current screen — it just can no longer be reached by id. That is
 *   what `peekCachedTerminal` is for, and reaching it is the difference between
 *   "the tab you are looking at" and "every tab".
 */

import type { Terminal } from '@xterm/xterm'
import { peekCachedTerminal } from '@/components/terminal/terminal-cache'
import { getTerminal } from '@/utils/terminal-registry'

/**
 * Cap on how many lines are read when a terminal reports no row count. Screen
 * rules only ever look at the last handful of lines, so this is headroom rather
 * than a target.
 */
const FALLBACK_SCREEN_LINES = 40

/**
 * Bottom-of-buffer text for an xterm instance.
 *
 * Returns plain text: xterm's cell buffer has already resolved escape
 * sequences, so unlike a raw byte tail there is nothing to strip and a redrawn
 * TUI frame reads as what the user actually sees.
 */
export function readScreenTailFromTerminal(terminal: Terminal, maxLines?: number): string {
  const buffer = terminal.buffer.active
  const wanted = maxLines ?? (terminal.rows > 0 ? terminal.rows : FALLBACK_SCREEN_LINES)
  const start = Math.max(0, buffer.length - wanted)

  const lines: string[] = []
  for (let i = start; i < buffer.length; i++) {
    lines.push(buffer.getLine(i)?.translateToString(true) ?? '')
  }
  return lines.join('\n')
}

/**
 * Bottom-of-buffer text for a terminal record, or `null` when it has no live
 * xterm to read.
 *
 * `null` is load-bearing: it means "no evidence", which
 * `deriveAgentTerminalState` must not confuse with an idle screen. A terminal
 * restored from a previous session but never opened, or evicted from the xterm
 * cache, lands here.
 */
export function readTerminalScreenTail(
  terminalId: string,
  ptyId: string | undefined | null,
  maxLines?: number
): string | null {
  const mounted = getTerminal(terminalId)
  if (mounted) return readScreenTailFromTerminal(mounted, maxLines)

  const cached = ptyId ? peekCachedTerminal(ptyId) : undefined
  if (cached) return readScreenTailFromTerminal(cached.terminal, maxLines)

  return null
}
