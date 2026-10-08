/**
 * Keeps each terminal's `agentSession` in step with the claude / codex / pi /
 * qin-code session running under its shell, so the persisted layout knows what
 * to reopen if the process is gone at the next restore.
 *
 * Polled rather than event-driven: nothing tells the app when an agent starts,
 * switches session (`/clear`, `/new`, `/resume`) or exits inside a shell. The
 * last poll before a machine shuts down is what a restore will see.
 */
import { useEffect } from 'react'
import {
  detectLiveAgentSessions,
  isSameAgentSession,
  toTerminalAgentSession
} from '@/lib/agents/live-agent-sessions'
import { terminalApi } from '@/lib/api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { useTerminalStore } from '@/stores/terminal-store'

const POLL_INTERVAL_MS = 15_000

export async function syncTerminalAgentSessions(): Promise<void> {
  const tracked = useTerminalStore.getState().terminals.filter((terminal) => terminal.ptyId)
  if (tracked.length === 0 || typeof terminalApi.list !== 'function') return
  const listed = await terminalApi.list()
  if (!listed.success) return
  const pidByPty = new Map(listed.data.map((status) => [status.id, status.pid]))
  const roots = tracked
    .map((terminal) => pidByPty.get(terminal.ptyId ?? ''))
    .filter((pid): pid is number => typeof pid === 'number' && pid > 0)
  const live = await detectLiveAgentSessions(roots)
  const byRoot = new Map(live.map((session) => [session.rootPid, session]))

  const store = useTerminalStore.getState()
  for (const terminal of tracked) {
    const pid = pidByPty.get(terminal.ptyId ?? '')
    // No live PTY to look under says nothing about the agent: keep what was
    // last seen, which is exactly what a restore after a crash needs.
    if (pid === undefined) continue
    const found = byRoot.get(pid)
    const next = found ? toTerminalAgentSession(found) : undefined
    const current = store.terminals.find((t) => t.id === terminal.id)?.agentSession
    if (!isSameAgentSession(current, next)) {
      store.setTerminalAgentSession(terminal.id, next)
    }
  }
}

export function useAgentSessionTracker(): void {
  useEffect(() => {
    if (!isTauriContext()) return
    let running = false
    const tick = (): void => {
      if (running) return
      running = true
      void syncTerminalAgentSessions()
        .catch((error) => console.warn('[agent-session-tracker] detection failed:', error))
        .finally(() => {
          running = false
        })
    }
    tick()
    const timer = setInterval(tick, POLL_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [])
}
