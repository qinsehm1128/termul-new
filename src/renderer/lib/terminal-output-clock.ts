/**
 * When each PTY last produced output, whether or not a renderer shows it.
 *
 * `hasActivity` on the terminal record is only maintained by a mounted
 * ConnectedTerminal, so it goes stale the moment the user leaves the workspace —
 * on the terminal board, every terminal looked idle. This clock is fed from the
 * app-wide data sidecar instead, and is deliberately not reactive: a store update
 * per chunk is exactly the cost the board must not add. Readers poll it.
 */
const lastOutputAt = new Map<string, number>()

export function noteTerminalOutput(ptyId: string, now: number = Date.now()): void {
  lastOutputAt.set(ptyId, now)
}

export function lastTerminalOutputAt(ptyId: string | undefined): number | undefined {
  return ptyId ? lastOutputAt.get(ptyId) : undefined
}

export function forgetTerminalOutput(ptyId: string): void {
  lastOutputAt.delete(ptyId)
}

/** Test-only: start from an empty clock. */
export function _resetTerminalOutputClockForTests(): void {
  lastOutputAt.clear()
}
