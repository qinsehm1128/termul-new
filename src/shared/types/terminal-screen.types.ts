/** Host-side screen observer preference. It never selects the PTY or renderer. */
export type TerminalScreenEngine = 'existing' | 'ghostty-vt'

/** Older snapshots/hosts and unknown future values retain existing behavior. */
export function resolveTerminalScreenEngine(
  project?: { terminalScreenEngine?: unknown } | null
): TerminalScreenEngine {
  return project?.terminalScreenEngine === 'ghostty-vt' ? 'ghostty-vt' : 'existing'
}
