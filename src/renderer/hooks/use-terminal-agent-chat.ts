import type { AgentChatSnapshot } from '@shared/types/agent-chat.types'
import { useCallback, useEffect, useRef, useState } from 'react'
import type { ChatMessage } from '../features/agent-session/stores/acp-store'
import type { ToolCall } from '../lib/acp-api'
import { agentChatApi, subscribeAgentChatUpdated } from '../lib/agent-chat-api'
import { logFrontendError } from '../lib/log-api'
import { isTauriContext } from '../lib/tauri-runtime'

type Snapshot = AgentChatSnapshot<ChatMessage, ToolCall>

/** Incremental reads replace updated messages by id, never duplicate them. */
export function mergeAgentChatSnapshot(previous: Snapshot | null, next: Snapshot): Snapshot {
  if (
    !previous ||
    next.reset ||
    previous.session_id !== next.session_id ||
    next.cursor < previous.cursor
  )
    return next
  const messages = new Map(previous.messages.map((message) => [message.id, message]))
  for (const message of next.messages) messages.set(message.id, message)
  return { ...next, messages: [...messages.values()] }
}

/** Metadata push is a latency optimization; polling also covers browser/reconnect gaps. */
export function useTerminalAgentChat(terminalId: string) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null)
  const [error, setError] = useState<string | null>(null)
  const refreshRef = useRef<() => Promise<void>>(async () => {})

  useEffect(() => {
    let disposed = false
    let current: Snapshot | null = null
    let inflight = false
    let refreshAgain = false
    let degraded = false
    const reportDegradation = () => {
      if (!degraded)
        void logFrontendError({
          source: 'terminal-agent-chat:snapshot',
          level: 'warn',
          message:
            'Terminal Agent chat snapshot unavailable or degraded; terminal fallback remains available.'
        })
      degraded = true
    }
    setSnapshot(null)
    setError(null)
    const refresh = async () => {
      if (disposed) return
      if (inflight) {
        refreshAgain = true
        return
      }
      inflight = true
      try {
        const next = await agentChatApi.snapshot(terminalId, current?.cursor)
        if (!disposed) {
          current = mergeAgentChatSnapshot(current, next)
          setSnapshot(current)
          setError(null)
          if (current.state === 'degraded') reportDegradation()
          else degraded = false
        }
      } catch (cause) {
        if (!disposed) {
          setError(cause instanceof Error ? cause.message : String(cause))
          reportDegradation()
        }
      } finally {
        inflight = false
        if (refreshAgain && !disposed) {
          refreshAgain = false
          void refresh()
        }
      }
    }
    refreshRef.current = refresh
    void refresh()
    const timer = window.setInterval(() => void refresh(), 2000)
    let unlisten: (() => void) | undefined
    if (isTauriContext()) {
      void subscribeAgentChatUpdated((payload) => {
        if (payload.terminal_id === terminalId) void refresh()
      })
        .then((off) => {
          if (disposed) off()
          else unlisten = off
        })
        .catch(() => {
          /* Polling remains available if metadata subscription fails. */
        })
    }
    return () => {
      disposed = true
      window.clearInterval(timer)
      unlisten?.()
    }
  }, [terminalId])

  const refresh = useCallback(() => refreshRef.current(), [])
  return { snapshot, error, refresh }
}
