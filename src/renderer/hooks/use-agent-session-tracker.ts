/** Hook identity is durable metadata; runtime activity belongs only to its store. */
import type { HookStateSyncResponse } from '@shared/types/agent-hooks.types'
import { useEffect } from 'react'
import { agentHooksApi } from '@/lib/agent-hooks-api'
import { hookStateSession } from '@/lib/agents/agent-hook-session'
import {
  detectLiveAgentSessions,
  isSameAgentSession,
  toTerminalAgentSession
} from '@/lib/agents/live-agent-sessions'
import { terminalApi } from '@/lib/api'
import { useAgentHookStateStore } from '@/stores/agent-hook-state-store'
import { useTerminalStore } from '@/stores/terminal-store'

const PROCESS_DISCOVERY_INTERVAL_MS = 15_000

/** Apply runtime first, then project only accepted identity changes into layout state. */
export function applyAgentHookStateSync(response: HookStateSyncResponse): void {
  const previous = useAgentHookStateStore.getState()
  previous.applySync(response)
  const runtime = useAgentHookStateStore.getState()
  if (runtime === previous) return
  for (const [key, state] of Object.entries(runtime.states)) {
    if (previous.states[key] === state) continue
    const store = useTerminalStore.getState()
    const terminal = store.terminals.find(
      (t) => t.ptyId === state.terminal_id && t.projectId === state.project_id
    )
    if (!terminal) continue
    const session = hookStateSession(state)
    if (session && !isSameAgentSession(terminal.agentSession, session)) {
      store.setTerminalAgentSession(terminal.id, session)
    }
  }
  // Ending or removing runtime does not erase the durable identity used for resume.
}

export async function syncTerminalAgentSessions(): Promise<void> {
  const tracked = useTerminalStore.getState().terminals.filter((terminal) => terminal.ptyId)
  if (tracked.length === 0 || typeof terminalApi.list !== 'function') return
  const listed = await terminalApi.list()
  if (!listed.success) return
  const snapshots = new Map(listed.data.map((status) => [status.id, status]))
  const roots = tracked
    .map((t) => snapshots.get(t.ptyId ?? '')?.pid)
    .filter((pid): pid is number => typeof pid === 'number' && pid > 0)
  const live = roots.length > 0 ? await detectLiveAgentSessions(roots) : []
  const byRoot = new Map(live.map((session) => [session.rootPid, session]))
  for (const terminal of tracked) {
    const pid = snapshots.get(terminal.ptyId ?? '')?.pid
    if (pid === undefined) continue
    const store = useTerminalStore.getState()
    const current = store.terminals.find((t) => t.id === terminal.id)
    if (!current || current.ptyId !== terminal.ptyId || current.agentSession?.source === 'hook')
      continue
    const found = byRoot.get(pid)
    const next = found
      ? { ...toTerminalAgentSession(found), source: found.source ?? ('poll' as const) }
      : undefined
    if (!isSameAgentSession(current.agentSession, next)) {
      store.setTerminalAgentSession(terminal.id, next)
    }
  }
}

export function useAgentSessionTracker(): void {
  useEffect(() => {
    let disposed = false
    let running = false
    const discover = (): void => {
      if (disposed || running) return
      running = true
      void syncTerminalAgentSessions()
        .catch(() => {
          /* process discovery is a fallback */
        })
        .finally(() => {
          running = false
        })
    }
    // B owns snapshot reconciliation, transport subscriptions and its single timer.
    const unsubscribe = agentHooksApi.subscribeState((response) => {
      if (!disposed) applyAgentHookStateSync(response)
    })
    discover()
    const interval = setInterval(discover, PROCESS_DISCOVERY_INTERVAL_MS)
    return () => {
      disposed = true
      clearInterval(interval)
      unsubscribe()
    }
  }, [])
}
