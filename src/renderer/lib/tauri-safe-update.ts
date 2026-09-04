import { useTerminalStore } from '@/stores/terminal-store'

export function hasActiveTerminalSessions(): boolean {
  const terminals = useTerminalStore.getState().terminals

  return terminals.some((terminal) => {
    // A terminal whose process is gone cannot lose work to an update restart.
    // Counting them kept warning the user about "running terminals" that had
    // already ended.
    if (
      terminal.healthStatus === 'hibernated' ||
      terminal.healthStatus === 'exited' ||
      terminal.healthStatus === 'crashed'
    ) {
      return false
    }
    return terminal.isHidden !== true
  })
}
