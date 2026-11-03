import type { TerminalInputModes } from '@shared/types/terminal-screen.types'
import type { Terminal } from '@xterm/xterm'

const MOUSE_PROTOCOLS = new Set([1000, 1002, 1003])

/**
 * Bytes that make a local xterm's input routing equal the host-tracked modes.
 *
 * Written into the renderer's own parser only (never to the PTY), so the
 * program sees nothing. Every mode is stated explicitly — OFF as well as ON —
 * because the point is to correct a renderer that drifted, in either
 * direction. The alternate-screen buffer is deliberately left alone: switching
 * buffers would blank or rewrite the visible screen, and it does not decide
 * who owns the mouse.
 */
export function buildInputModeResyncSequence(modes: TerminalInputModes): string {
  const seqs: string[] = []
  // Any mouse-protocol reset clears xterm's single active protocol.
  seqs.push('\x1b[?1000l')
  if (modes.mouseTracking !== null && MOUSE_PROTOCOLS.has(modes.mouseTracking)) {
    seqs.push(`\x1b[?${modes.mouseTracking}h`)
  }
  if (modes.mouseEncoding === 1006) {
    seqs.push('\x1b[?1006h')
  } else if (modes.mouseTracking === null) {
    // Only drop the encoding when mouse reporting is off: the host does not
    // track 1016 (SGR pixels), so leave an encoding alone while capture is on.
    seqs.push('\x1b[?1006l')
  }
  seqs.push(modes.bracketedPaste ? '\x1b[?2004h' : '\x1b[?2004l')
  seqs.push(modes.applicationCursor ? '\x1b[?1h' : '\x1b[?1l')
  seqs.push(modes.focusEvents ? '\x1b[?1004h' : '\x1b[?1004l')
  seqs.push(modes.applicationKeypad ? '\x1b=' : '\x1b>')
  return seqs.join('')
}

/** xterm's own name for a host mouse protocol number. */
export function hostMouseTrackingName(mode: number | null): 'none' | 'vt200' | 'drag' | 'any' {
  switch (mode) {
    case 1000:
      return 'vt200'
    case 1002:
      return 'drag'
    case 1003:
      return 'any'
    default:
      return 'none'
  }
}

/** Apply host modes to the local renderer; resolves once xterm parsed them. */
export function applyInputModesToRenderer(
  terminal: Pick<Terminal, 'write'>,
  modes: TerminalInputModes
): Promise<void> {
  return new Promise((resolve) => terminal.write(buildInputModeResyncSequence(modes), resolve))
}
