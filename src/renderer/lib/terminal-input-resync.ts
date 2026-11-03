import type { TerminalInputModes } from '@shared/types/terminal-screen.types'
import { readTerminalInputModes } from '@/lib/terminal-api'
import { useTerminalStore } from '@/stores/terminal-store'
import { applyInputModesToRenderer } from '@/utils/terminal-input-modes'
import { getTerminal } from '@/utils/terminal-registry'

export type InputModeResyncResult =
  | {
      status: 'applied'
      /** xterm's mouse mode before the resync ('none' | 'vt200' | 'drag' | 'any' | ...). */
      rendererBefore: string
      rendererAfter: string
      host: TerminalInputModes
    }
  | { status: 'unavailable'; reason: 'no-renderer' | 'no-claim' | 'host-denied' | 'unsupported' }

/**
 * Re-take the program's input routing in this renderer: read the host's
 * tracked modes (claim-gated) and apply them to the local xterm only. The PTY
 * and the program receive nothing, so a running TUI is never disturbed.
 *
 * Used after a renderer re-attaches to a long-lived PTY (the program set its
 * mouse capture long before this xterm existed) and from the manual
 * "resync input takeover" button.
 */
export async function resyncRendererInputModes(ptyId: string): Promise<InputModeResyncResult> {
  const terminal = getTerminal(ptyId)
  if (!terminal) return { status: 'unavailable', reason: 'no-renderer' }
  const claim = useTerminalStore.getState().findTerminalByPtyId(ptyId)?.claim
  if (!claim) return { status: 'unavailable', reason: 'no-claim' }
  const result = await readTerminalInputModes(ptyId, claim)
  if (!result.success) return { status: 'unavailable', reason: 'host-denied' }
  if (!result.data) return { status: 'unavailable', reason: 'unsupported' }
  // The renderer may have been replaced while the host answered.
  if (getTerminal(ptyId) !== terminal) return { status: 'unavailable', reason: 'no-renderer' }
  const rendererBefore = terminal.modes.mouseTrackingMode
  await applyInputModesToRenderer(terminal, result.data)
  return {
    status: 'applied',
    rendererBefore,
    rendererAfter: terminal.modes.mouseTrackingMode,
    host: result.data
  }
}
