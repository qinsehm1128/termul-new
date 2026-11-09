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

/** Reconcile cached identity after either runtime updates or PTY adoption. */
export function reconcileAgentHookIdentities(): void {
  for (const state of Object.values(useAgentHookStateStore.getState().states)) {
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

/** Apply runtime first; durable identity equality prevents runtime-only autosaves. */
export function applyAgentHookStateSync(response: HookStateSyncResponse): void {
  useAgentHookStateStore.getState().applySync(response)
  reconcileAgentHookIdentities()
}

export async function syncTerminalAgentSessions(): Promise<void> {
  // Filter before PTY listing / process detection, not only after paying their cost.
  const tracked = useTerminalStore
    .getState()
    .terminals.filter((terminal) => terminal.ptyId && terminal.agentSession?.source !== 'hook')
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
      if (disposed || running || document.hidden) return
      running = true
      void syncTerminalAgentSessions()
        .catch(() => {
          /* process discovery is a fallback */
        })
        .finally(() => {
          running = false
        })
    }
    const needsLegacyDiscovery = () =>
      useTerminalStore
        .getState()
        .terminals.some((terminal) => terminal.ptyId && terminal.agentSession?.source !== 'hook')
    // The singleton owns the only timer: disconnected recovery or, while push
    // is healthy, a 15s legacy fallback only for terminals without hook identity.
    const unsubscribe = agentHooksApi.subscribeState(
      (response) => {
        if (!disposed) applyAgentHookStateSync(response)
      },
      discover,
      needsLegacyDiscovery
    )
    const unsubscribeTerminals = useTerminalStore.subscribe((store, previous) => {
      const attachmentsChanged =
        store.terminals.length !== previous.terminals.length ||
        store.terminals.some((terminal, index) => {
          const old = previous.terminals[index]
          return (
            terminal.id !== old?.id ||
            terminal.ptyId !== old.ptyId ||
            terminal.projectId !== old.projectId
          )
        })
      if (disposed) return
      if (attachmentsChanged) {
        reconcileAgentHookIdentities()
        discover()
      }
      if (
        attachmentsChanged ||
        store.terminals.some(
          (terminal, index) =>
            terminal.agentSession?.source !== previous.terminals[index]?.agentSession?.source
        )
      )
        unsubscribe.refreshFallback?.()
    })
    reconcileAgentHookIdentities()
    discover()
    unsubscribe.refreshFallback?.()
    return () => {
      disposed = true
      unsubscribeTerminals()
      unsubscribe()
    }
  }, [])
}
