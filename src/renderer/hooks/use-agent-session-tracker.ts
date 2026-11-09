/** Hook lifecycle updates are authoritative; process discovery remains a legacy fallback. */
import type { AgentHookEvent } from '@shared/types/agent-hooks.types'
import { useEffect } from 'react'
import { agentHooksApi } from '@/lib/agent-hooks-api'
import { hookEventSession } from '@/lib/agents/agent-hook-session'
import { hookTerminalState } from '@/lib/agents/agent-hook-state'
import {
  detectLiveAgentSessions,
  isSameAgentSession,
  toTerminalAgentSession
} from '@/lib/agents/live-agent-sessions'
import { terminalApi } from '@/lib/api'
import { listen } from '@/lib/tauri-event'
import { isTauriContext } from '@/lib/tauri-runtime'
import { useTerminalStore } from '@/stores/terminal-store'

export const AGENT_HOOK_EVENT_POLL_MS = 750
const POLL_INTERVAL_MS = 15_000

export function applyAgentHookEvent(event: AgentHookEvent): void {
  const store = useTerminalStore.getState()
  // Events carry the immutable host PTY id, not a renderer tab id.
  const terminal = store.terminals.find(
    (t) => t.ptyId === event.terminal_id && t.projectId === event.project_id
  )
  if (!terminal) return
  const session = hookEventSession(event)
  if (!session) return
  if (event.kind === 'session_end') {
    if (
      terminal.agentSession?.source === 'hook' &&
      terminal.agentSession.sessionId === event.session_id
    ) {
      store.setTerminalAgentSession(terminal.id, undefined)
      store.setTerminalAgentState(terminal.id, 'idle')
    }
    return
  }
  store.setTerminalAgentSession(terminal.id, session)
  const state = hookTerminalState(session)
  if (state) store.setTerminalAgentState(terminal.id, state)
}

export async function syncTerminalAgentSessions(): Promise<void> {
  const tracked = useTerminalStore.getState().terminals.filter((terminal) => terminal.ptyId)
  if (tracked.length === 0 || typeof terminalApi.list !== 'function') return
  const listed = await terminalApi.list()
  if (!listed.success) return
  const snapshots = new Map(listed.data.map((status) => [status.id, status]))
  const fallback = tracked
  const roots = fallback
    .map((t) => snapshots.get(t.ptyId ?? '')?.pid)
    .filter((pid): pid is number => typeof pid === 'number' && pid > 0)
  const live = roots.length > 0 ? await detectLiveAgentSessions(roots) : []
  const byRoot = new Map(live.map((session) => [session.rootPid, session]))
  for (const terminal of fallback) {
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
    if (!isSameAgentSession(current.agentSession, next))
      store.setTerminalAgentSession(terminal.id, next)
    const state = hookTerminalState(next)
    if (state) store.setTerminalAgentState(terminal.id, state)
  }
}

export function useAgentSessionTracker(): void {
  useEffect(() => {
    let disposed = false
    let running = false
    let discoveryRunning = false
    let cursor = 0
    let retryAfter: number | undefined
    let lastSnapshotAt = -Infinity
    const seen = new Map<string, number>()
    const receive = (event: AgentHookEvent): void => {
      if (disposed || !Number.isSafeInteger(event.sequence) || event.sequence < 1) return
      cursor = Math.max(cursor, event.sequence)
      const key = `${event.project_id}\0${event.terminal_id}`
      if ((seen.get(key) ?? 0) >= event.sequence) return
      seen.set(key, event.sequence)
      applyAgentHookEvent(event)
    }
    const tick = (): void => {
      if (disposed) return
      if (!discoveryRunning && Date.now() - lastSnapshotAt >= POLL_INTERVAL_MS) {
        discoveryRunning = true
        lastSnapshotAt = Date.now()
        void syncTerminalAgentSessions()
          .catch((error) => console.warn('[agent-session-tracker] detection failed:', error))
          .finally(() => {
            discoveryRunning = false
          })
      }
      if (running) return
      running = true
      const requestedAfter = cursor
      // A failed request retries initial catchup, but retains its generation boundary.
      const generationAfter = requestedAfter || retryAfter || 0
      void agentHooksApi
        .events(requestedAfter)
        .then((events) => {
          if (disposed) return
          const maxSequence = events.reduce((max, event) => Math.max(max, event.sequence), 0)
          // Compare against the request, not a cursor advanced by concurrent live events.
          if (events.length > 0 && maxSequence < generationAfter) {
            cursor = 0
            seen.clear()
            const store = useTerminalStore.getState()
            for (const terminal of store.terminals) {
              if (terminal.ptyId && terminal.agentSession?.source === 'hook') {
                store.setTerminalAgentSession(terminal.id, undefined)
                store.setTerminalAgentState(terminal.id, 'idle')
              }
            }
          }
          if (events.length > 0) retryAfter = undefined
          for (const event of events.sort((a, b) => a.sequence - b.sequence)) receive(event)
        })
        .catch((error: unknown) => {
          if (disposed) return
          const message = error instanceof Error ? error.message : String(error)
          // Older hosts keep their process-discovery fallback; transport outages retry catchup.
          if (/unknown command|command.*not found|unsupported|not supported|\b404\b/i.test(message))
            return
          retryAfter = generationAfter
          cursor = 0
        })
        .finally(() => {
          running = false
        })
    }
    let unlisten: (() => void) | undefined
    if (isTauriContext()) {
      void listen<AgentHookEvent>('agent-hook-event', ({ payload }) => receive(payload))
        .then((stop) => {
          if (disposed) stop()
          else unlisten = stop
        })
        .catch(() => {})
    }
    tick()
    const timer = setInterval(tick, AGENT_HOOK_EVENT_POLL_MS)
    return () => {
      disposed = true
      clearInterval(timer)
      unlisten?.()
    }
  }, [])
}
