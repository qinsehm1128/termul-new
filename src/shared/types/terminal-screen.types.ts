/** Host-side screen observer preference. It never selects the PTY or renderer. */
export type TerminalScreenEngine = 'existing' | 'ghostty-vt'

/** Older snapshots/hosts and unknown future values retain existing behavior. */
export function resolveTerminalScreenEngine(
  project?: { terminalScreenEngine?: unknown } | null
): TerminalScreenEngine {
  return project?.terminalScreenEngine === 'ghostty-vt' ? 'ghostty-vt' : 'existing'
}

export type ScreenObservationState =
  | 'disabled'
  | 'starting'
  | 'catching-up'
  | 'ready'
  | 'degraded'
  | 'unsupported'

export type ScreenObservationReason =
  | 'helper-missing'
  | 'identity-mismatch'
  | 'overflow'
  | 'timeout'
  | 'crash'
  | 'protocol'
  | 'size-limit'
  | 'legacy-host'

/** Host-issued state only. A project preference must never construct a ready state. */
export interface ScreenObservationStatus {
  terminalScreenEngine: TerminalScreenEngine
  state: ScreenObservationState
  reason: ScreenObservationReason | null
  capturedSeq: number
  appliedSeq: number
  observerBuildId: string | null
}

/** Claim/scope-protected, bounded host screen evidence. Not a renderer transcript. */
export interface TerminalScreenReadResult {
  status: ScreenObservationStatus
  text: string | null
  appliedSeq: number | null
  truncated: boolean
}

export function legacyScreenObservation(): ScreenObservationStatus {
  return {
    terminalScreenEngine: 'existing',
    state: 'unsupported',
    reason: 'legacy-host',
    capturedSeq: 0,
    appliedSeq: 0,
    observerBuildId: null
  }
}

/** Evidence is usable only when the host explicitly reports a consistent ready barrier. */
export function isScreenObservationReady(status: ScreenObservationStatus): boolean {
  return (
    status.terminalScreenEngine === 'ghostty-vt' &&
    status.state === 'ready' &&
    status.reason === null &&
    Number.isSafeInteger(status.capturedSeq) &&
    Number.isSafeInteger(status.appliedSeq) &&
    status.appliedSeq >= 0 &&
    status.capturedSeq === status.appliedSeq
  )
}

/** Fail closed on a stale/inconsistent reply; never turn unavailable output into evidence. */
export function usableHostScreenText(result: TerminalScreenReadResult): string | null {
  return isScreenObservationReady(result.status) &&
    result.appliedSeq === result.status.appliedSeq &&
    typeof result.text === 'string'
    ? result.text
    : null
}

/** Unknown/malformed hosts are explicitly unavailable, never optimistically ready. */
export function normalizeScreenObservation(value: unknown): ScreenObservationStatus {
  if (value === undefined || value === null) return legacyScreenObservation()
  const raw = typeof value === 'object' ? (value as Record<string, unknown>) : {}
  const states: readonly unknown[] = [
    'disabled',
    'starting',
    'catching-up',
    'ready',
    'degraded',
    'unsupported'
  ]
  const reasons: readonly unknown[] = [
    null,
    'helper-missing',
    'identity-mismatch',
    'overflow',
    'timeout',
    'crash',
    'protocol',
    'size-limit',
    'legacy-host'
  ]
  if (
    !['existing', 'ghostty-vt'].includes(raw.terminalScreenEngine as string) ||
    !states.includes(raw.state) ||
    !reasons.includes(raw.reason) ||
    !Number.isSafeInteger(raw.capturedSeq) ||
    !Number.isSafeInteger(raw.appliedSeq) ||
    (raw.capturedSeq as number) < 0 ||
    (raw.appliedSeq as number) < 0 ||
    !(raw.observerBuildId === null || typeof raw.observerBuildId === 'string')
  ) {
    return {
      ...legacyScreenObservation(),
      terminalScreenEngine: resolveTerminalScreenEngine(raw),
      reason: 'protocol'
    }
  }
  const status = value as ScreenObservationStatus
  return status.state === 'ready' && !isScreenObservationReady(status)
    ? { ...status, state: 'degraded', reason: 'protocol' }
    : status
}
