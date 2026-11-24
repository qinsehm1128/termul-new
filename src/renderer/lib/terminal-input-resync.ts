import type { TerminalInputModes } from '@shared/types/terminal-screen.types'
import { logFrontendError } from '@/lib/log-api'
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

/**
 * Proof supplied by the owning terminal view for an *automatic* claim recovery
 * after a reattach. Every flag is a fact only the mounted component can know;
 * the store-side facts are re-checked in {@link evaluateAutomaticClaimRecovery}.
 *
 * Automatic recovery is deliberately narrower than the explicit user takeover:
 * a rotation invalidates the previous credential generation, which tears down
 * any other desktop stream that was using it. The caller therefore has to prove
 * that this mount is the attached, visible desktop view, not a hidden pane, a
 * companion phone surface, or the mobile web shell.
 */
export interface AutomaticClaimRecoveryEvidence {
  /** The host accepted this mount's renderer reference (`addRendererRef`). */
  rendererRefAttached: boolean
  /** This terminal's tab is the active, on-screen surface in this window. */
  visible: boolean
  /** Running in the browser/mobile web shell rather than the desktop window. */
  mobileWebShell: boolean
  /** A companion phone surface owns this terminal's geometry. */
  companionPhoneSurface: boolean
  /** The host display mode parked this terminal for a phone. */
  parkedByPhone: boolean
}

export interface InputModeResyncOptions {
  /**
   * Allow the host-authorized claim recovery when this window holds none.
   *
   * Only an explicit user takeover should rotate a claim without further
   * evidence: doing it from the automatic post-restore call would silently
   * revoke the phone/companion's credential on every renderer reload. Defaults
   * to false.
   */
  recoverClaim?: boolean
  /**
   * Automatic post-reattach recovery evidence. Honored only when no claim is
   * held locally and {@link evaluateAutomaticClaimRecovery} accepts every flag
   * plus the live store record. Never rotates a claim for a hidden, detached,
   * phone-owned or non-desktop view.
   */
  autoRecoverClaim?: AutomaticClaimRecoveryEvidence
}

type AutomaticClaimRecoveryRefusal =
  | 'EVIDENCE_REJECTED'
  | 'NO_RECORD'
  | 'NOT_RUNNING'
  | 'NOT_VISIBLE'

/**
 * Decide whether an automatic (not user-initiated) claim recovery may run.
 *
 * Why this is safe to rotate:
 *
 * - The runtime must be the desktop Tauri app. `supportsTerminalInputModes()`
 *   is the existing transport predicate; the browser/mobile web shell and the
 *   remote/se-server surfaces have no claim-gated input-mode reader and return
 *   `unsupported` before this point, so a second browser window/device can
 *   never rotate a desktop claim through this path.
 * - The caller must hold the host renderer reference for this mount, be the
 *   active tab, and not be the mobile web shell. A hidden or not-yet-attached
 *   view has no business replacing the credential of the visible owner.
 * - No companion phone surface may own the terminal (`surfaceActive` in phone
 *   mode, or the host display mode parked it). The desktop app is single-window
 *   (`tauri.conf.json` declares only `main`); the only other viewers are these
 *   claim-free phone/remote streams, whose input ownership is exactly the
 *   companion/display-mode predicate. Refusing here keeps their takeover
 *   intact.
 * - The store record must still be the live, running record bound to this PTY
 *   and not hidden. A replaced/rebound record or a terminal that already
 *   exited has nothing worth recovering.
 *
 * The host remains the authority: `acquireTerminalClaim` re-validates the
 * Conversation (or quick-terminal/project restart-adoption) scope and rotates
 * only after that validation. A denial leaves the record untouched.
 */
function evaluateAutomaticClaimRecovery(
  ptyId: string,
  evidence: AutomaticClaimRecoveryEvidence
): AutomaticClaimRecoveryRefusal | null {
  if (
    !evidence.rendererRefAttached ||
    !evidence.visible ||
    evidence.mobileWebShell ||
    evidence.companionPhoneSurface ||
    evidence.parkedByPhone
  ) {
    // Expected refusal: a phone/hidden/shell mount is a normal state, not a
    // failure, so it is not logged.
    return 'EVIDENCE_REJECTED'
  }

  const record = useTerminalStore.getState().findTerminalByPtyId(ptyId)
  if (!record || record.ptyId !== ptyId) return 'NO_RECORD'
  if (record.healthStatus !== 'running') return 'NOT_RUNNING'
  if (record.viewState === 'hidden' || record.isHidden) return 'NOT_VISIBLE'
  return null
}

/**
 * Log an automatic recovery failure with a stable code only.
 *
 * Never include the claim, terminal output, cwd, project id, record id or any
 * other host detail: this line is written to the persistent production log.
 */
function logAutomaticClaimRecovery(
  outcome: 'skipped' | 'denied' | 'unavailable',
  code: AutomaticClaimRecoveryRefusal | 'HOST_DENIED' | 'NO_CLAIM_AVAILABLE' | 'UNSUPPORTED'
): void {
  void logFrontendError({
    level: 'warn',
    source: 'terminal-input-resync.auto',
    message: `code=AUTO_INPUT_CLAIM_RECOVERY_${outcome.toUpperCase()}_${code}`
  })
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
  let automaticRecovery = false
  if (!claim) {
    let recoveryMode: 'explicit' | 'automatic' | null = null
    if (options.recoverClaim) {
      recoveryMode = 'explicit'
    } else if (options.autoRecoverClaim) {
      const refusal = evaluateAutomaticClaimRecovery(ptyId, options.autoRecoverClaim)
      if (refusal) {
        if (refusal !== 'EVIDENCE_REJECTED') {
          logAutomaticClaimRecovery('skipped', refusal)
        }
        return { status: 'unavailable', reason: 'no-claim' }
      }
      recoveryMode = 'automatic'
    }
    if (!recoveryMode) return { status: 'unavailable', reason: 'no-claim' }
    automaticRecovery = recoveryMode === 'automatic'

    // Claims are renderer memory only. A record reattached by `watch` after a
    // reload is live and healthy yet has no credential, so re-obtain one from
    // the host through the same Conversation-scoped resume path used for cold
    // starts. Live-only: this xterm already painted the watch replay.
    const recovered = await store.acquireTerminalClaim(ptyId)
    if (recovered.status === 'unavailable') {
      if (automaticRecovery) {
        logAutomaticClaimRecovery('unavailable', 'NO_CLAIM_AVAILABLE')
      }
      return { status: 'unavailable', reason: 'no-claim' }
    }
    if (recovered.status === 'denied') {
      if (automaticRecovery) {
        logAutomaticClaimRecovery('denied', 'HOST_DENIED')
      }
      return { status: 'unavailable', reason: 'host-denied' }
    }
    claim = recovered.claim
  }
  // The renderer may have been replaced while the claim was acquired.
  if (getTerminal(ptyId) !== terminal) return { status: 'unavailable', reason: 'no-renderer' }

  const result = await readTerminalInputModes(ptyId, claim)
  if (!result.success) {
    if (automaticRecovery) logAutomaticClaimRecovery('denied', 'HOST_DENIED')
    return { status: 'unavailable', reason: 'host-denied' }
  }
  if (!result.data) {
    if (automaticRecovery) logAutomaticClaimRecovery('unavailable', 'UNSUPPORTED')
    return { status: 'unavailable', reason: 'unsupported' }
  }
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
