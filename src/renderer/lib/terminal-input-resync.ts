import type { TerminalInputModes } from '@shared/types/terminal-screen.types'
import { readTerminalInputModes, supportsTerminalInputModes } from '@/lib/terminal-api'
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

export interface InputModeResyncOptions {
  /**
   * Allow the host-authorized claim recovery when this window holds none.
   *
   * Only an explicit user takeover should rotate a claim: doing it from the
   * automatic post-restore call would silently revoke the phone/companion's
   * credential on every renderer reload. Defaults to false.
   */
  recoverClaim?: boolean
}

/**
 * Re-take the program's input routing in this renderer: read the host's
 * tracked modes (claim-gated) and apply them to the local xterm only. The PTY
 * and the program receive nothing, so a running TUI is never disturbed.
 *
 * Used after a renderer re-attaches to a long-lived PTY (the program set its
 * mouse capture long before this xterm existed) and from the manual
 * "resync input takeover" button.
 */
export async function resyncRendererInputModes(
  ptyId: string,
  options: InputModeResyncOptions = {}
): Promise<InputModeResyncResult> {
  const terminal = getTerminal(ptyId)
  if (!terminal) return { status: 'unavailable', reason: 'no-renderer' }
  // The browser/shared-live and se-server transports have no claim-gated
  // input-mode reader; their screen reads use the connection's Conversation
  // scope instead. Never rotate a credential for an action that cannot finish.
  if (!supportsTerminalInputModes()) return { status: 'unavailable', reason: 'unsupported' }

  const store = useTerminalStore.getState()
  let claim = store.findTerminalByPtyId(ptyId)?.claim
  if (!claim) {
    if (!options.recoverClaim) return { status: 'unavailable', reason: 'no-claim' }
    // Claims are renderer memory only. A record reattached by `watch` after a
    // reload is live and healthy yet has no credential, so re-obtain one from
    // the host through the same Conversation-scoped resume path used for cold
    // starts. Live-only: this xterm already painted the watch replay.
    const recovered = await store.acquireTerminalClaim(ptyId)
    if (recovered.status === 'unavailable') {
      return { status: 'unavailable', reason: 'no-claim' }
    }
    if (recovered.status === 'denied') {
      return { status: 'unavailable', reason: 'host-denied' }
    }
    claim = recovered.claim
  }
  // The renderer may have been replaced while the claim was acquired.
  if (getTerminal(ptyId) !== terminal) return { status: 'unavailable', reason: 'no-renderer' }

  const result = await readTerminalInputModes(ptyId, claim)
  if (!result.success) return { status: 'unavailable', reason: 'host-denied' }
  if (!result.data) return { status: 'unavailable', reason: 'unsupported' }
  // Applying modes must not race a renderer swap either.
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
