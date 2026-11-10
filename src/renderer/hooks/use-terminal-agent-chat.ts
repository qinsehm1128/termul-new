import type { AgentChatSnapshot } from '@shared/types/agent-chat.types'
import { useCallback, useEffect, useRef, useState } from 'react'
import type { ChatMessage } from '../features/agent-session/stores/acp-store'
import type { ToolCall } from '../lib/acp-api'
import {
  type AgentChatAbsence,
  agentChatAbsence,
  agentChatApi,
  agentChatErrorCode,
  subscribeAgentChatUpdated
} from '../lib/agent-chat-api'
import { logFrontendError } from '../lib/log-api'
import { isTauriContext } from '../lib/tauri-runtime'

type Snapshot = AgentChatSnapshot<ChatMessage, ToolCall>
export const AGENT_CHAT_MAX_MESSAGES = 1024
export const AGENT_CHAT_MAX_BYTES = 16 * 1024 * 1024
export const AGENT_CHAT_POLL_MS = 2000
export const AGENT_CHAT_MAX_POLL_MS = 10000

/**
 * Back off consecutive failures: the first retry keeps the base cadence, then
 * 2s doubles to the 10s ceiling. Recovery resets to the base cadence.
 */
export function agentChatPollDelay(failures: number): number {
  if (failures <= 0) return AGENT_CHAT_POLL_MS
  return Math.min(AGENT_CHAT_POLL_MS * 2 ** (failures - 1), AGENT_CHAT_MAX_POLL_MS)
}

function sequence(message: ChatMessage): number | undefined {
  return (message as ChatMessage & { seq?: number }).seq
}

function toolReference(message: ChatMessage): string | undefined {
  return (message as ChatMessage & { tool_call_id?: string }).tool_call_id
}

function mergeTools(previous: ToolCall[], next: ToolCall[], olderPage: boolean): ToolCall[] {
  const tools = new Map(previous.map((tool) => [tool.toolCallId, tool]))
  for (const tool of next) {
    const current = tools.get(tool.toolCallId)
    if (
      !current ||
      (tool.seq ?? 0) > (current.seq ?? 0) ||
      (!olderPage && (tool.seq ?? 0) === (current.seq ?? 0))
    )
      tools.set(tool.toolCallId, tool)
  }
  return [...tools.values()]
}

/** Bound complete message/tool units together; marker references, not latest tool seq, own reachability. */
function bounded(snapshot: Snapshot, keepOlder = false): Snapshot {
  const encoder = new TextEncoder()
  const size = (record: unknown) => encoder.encode(JSON.stringify(record)).byteLength
  const messageSizes = new Map(snapshot.messages.map((message) => [message.id, size(message)]))
  const toolSizes = new Map(snapshot.tool_calls.map((tool) => [tool.toolCallId, size(tool)]))
  const hasMarkers = snapshot.messages.some((message) => toolReference(message) !== undefined)
  let messages = snapshot.messages
  const reachableTools = () => {
    const refs = new Set(messages.map(toolReference).filter((id) => id !== undefined))
    if (hasMarkers) return snapshot.tool_calls.filter((tool) => refs.has(tool.toolCallId))
    // Older servers / fixtures have no explicit marker metadata. Keep only their seq window when known.
    const seqs = messages.map(sequence).filter((seq): seq is number => seq !== undefined)
    if (seqs.length) {
      const first = Math.min(...seqs),
        last = Math.max(...seqs)
      return snapshot.tool_calls.filter(
        (tool) => tool.seq === undefined || (tool.seq >= first && tool.seq <= last)
      )
    }
    return snapshot.tool_calls
  }
  let tools = reachableTools()
  const bytes = () =>
    messages.reduce((sum, message) => sum + (messageSizes.get(message.id) ?? 0), 0) +
    tools.reduce((sum, tool) => sum + (toolSizes.get(tool.toolCallId) ?? 0), 0)
  while (
    messages.length > 1 &&
    (messages.length > AGENT_CHAT_MAX_MESSAGES || bytes() > AGENT_CHAT_MAX_BYTES)
  ) {
    messages = keepOlder ? messages.slice(0, -1) : messages.slice(1)
    tools = reachableTools()
  }
  // Legacy tool-only snapshots have no marker unit. Bound those too, allowing one complete tool + message.
  while (
    tools.length > 1 &&
    (tools.length > AGENT_CHAT_MAX_MESSAGES || bytes() > AGENT_CHAT_MAX_BYTES)
  ) {
    tools = keepOlder ? tools.slice(0, -1) : tools.slice(1)
  }
  if (messages.length === snapshot.messages.length && tools.length === snapshot.tool_calls.length)
    return snapshot
  return {
    ...snapshot,
    messages,
    tool_calls: tools,
    older_cursor:
      !keepOlder && messages.length < snapshot.messages.length
        ? (sequence(messages[0]) ?? snapshot.older_cursor)
        : snapshot.older_cursor
  }
}

/** Live snapshots merge updates by id; an older page never changes the live polling cursor. */
export function mergeAgentChatSnapshot(
  previous: Snapshot | null,
  next: Snapshot,
  olderPage = false
): Snapshot {
  if (!previous || next.reset || previous.session_id !== next.session_id) {
    // Stale page results must not resurrect a session replaced while the request was in flight.
    if (olderPage) return previous ?? bounded(next)
    return bounded(next)
  }
  if (olderPage) {
    const messages = new Map(next.messages.map((message) => [message.id, message]))
    // Keep newer live content when a page overlaps a message updated by polling.
    for (const message of previous.messages) messages.set(message.id, message)
    return bounded(
      {
        ...previous,
        messages: [...messages.values()],
        tool_calls: mergeTools(previous.tool_calls, next.tool_calls, true),
        older_cursor: next.older_cursor
      },
      true
    )
  }
  // The terminal cursor is monotonic; delayed live replies cannot roll it backwards.
  if (next.cursor < previous.cursor) return previous
  const messages = new Map(previous.messages.map((message) => [message.id, message]))
  for (const message of next.messages) messages.set(message.id, message)
  return bounded({
    ...next,
    messages: [...messages.values()],
    tool_calls: mergeTools(previous.tool_calls, next.tool_calls, false),
    older_cursor: previous.older_cursor
  })
}

/** Metadata push is a latency optimization; polling also covers browser/reconnect gaps. */
export function useTerminalAgentChat(terminalId: string) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [absence, setAbsence] = useState<AgentChatAbsence | null>(null)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [browsingOlder, setBrowsingOlder] = useState(false)
  const refreshRef = useRef<() => Promise<void>>(async () => {})
  const latestRef = useRef<() => Promise<void>>(async () => {})
  const olderRef = useRef<() => Promise<void>>(async () => {})

  useEffect(() => {
    let disposed = false
    let current: Snapshot | null = null
    let inflight = false
    let paging = false
    let browsing = false
    let returningLatest = false
    let epoch = 0
    let refreshAgain = false
    let degraded = false
    let failures = 0
    let loggedCode: string | null = null
    let timer: ReturnType<typeof setTimeout> | undefined
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
    /** One log line per code transition, never one per poll tick. */
    const noteUnavailable = (code: string) => {
      if (loggedCode === code) return
      loggedCode = code
      if (code === 'AGENT_CHAT_NO_HOOK' || code === 'UNSUPPORTED_AGENT_CHAT')
        void logFrontendError({
          source: 'terminal-agent-chat:snapshot',
          level: 'warn',
          message: `Terminal Agent chat has no transcript to show (${code}); the empty state is shown and the terminal remains available.`
        })
      else reportDegradation()
    }
    const schedule = () => {
      if (disposed) return
      timer = setTimeout(() => {
        void refresh().finally(schedule)
      }, agentChatPollDelay(failures))
    }
    setSnapshot(null)
    setError(null)
    setAbsence(null)
    setLoadingOlder(false)
    setBrowsingOlder(false)
    const refresh = async () => {
      if (disposed || browsing) return
      if (inflight) {
        refreshAgain = true
        return
      }
      inflight = true
      const requestEpoch = epoch
      try {
        const next = await agentChatApi.snapshot(terminalId, current?.cursor)
        if (!disposed && !browsing && requestEpoch === epoch) {
          if (next.reset || current?.session_id !== next.session_id) epoch++
          current = mergeAgentChatSnapshot(current, next)
          setSnapshot(current)
          setError(null)
          setAbsence(null)
          failures = 0
          loggedCode = null
          if (current.state === 'degraded') reportDegradation()
          else degraded = false
        }
      } catch (cause) {
        if (!disposed) {
          // No hooked agent is a normal state: show the empty view, not an error.
          const absent = agentChatAbsence(cause)
          setAbsence(absent)
          setError(absent ? null : cause instanceof Error ? cause.message : String(cause))
          noteUnavailable(agentChatErrorCode(cause))
          failures += 1
        }
      } finally {
        inflight = false
        if (refreshAgain && !disposed) {
          refreshAgain = false
          void refresh()
        }
      }
    }
    const loadOlder = async () => {
      if (disposed || paging || returningLatest || current?.older_cursor == null) return
      paging = true
      if (!browsing) epoch++
      const wasBrowsing = browsing
      browsing = true
      setBrowsingOlder(true)
      setLoadingOlder(true)
      const requestedEpoch = epoch
      const session = current.session_id
      const before = current.older_cursor
      try {
        const page = await agentChatApi.snapshot(terminalId, undefined, before)
        if (!disposed && epoch === requestedEpoch && current?.session_id === session) {
          if (page.terminal_id !== terminalId || (!page.reset && page.session_id !== session)) {
            await returnLatest()
            return
          }
          if (page.reset) {
            // The backend reset fence turns a stale before into a fresh recent snapshot.
            epoch++
            browsing = false
            setBrowsingOlder(false)
            current = mergeAgentChatSnapshot(current, page)
          } else current = mergeAgentChatSnapshot(current, page, true)
          setSnapshot(current)
          setError(null)
        }
      } catch (cause) {
        if (!disposed && epoch === requestedEpoch) {
          browsing = wasBrowsing
          setBrowsingOlder(wasBrowsing)
          setError(cause instanceof Error ? cause.message : String(cause))
        }
      } finally {
        paging = false
        if (!disposed) setLoadingOlder(false)
      }
    }
    const returnLatest = async () => {
      if (disposed || returningLatest) return
      returningLatest = true
      epoch++
      const requestedEpoch = epoch
      try {
        const next = await agentChatApi.snapshot(terminalId)
        if (!disposed && epoch === requestedEpoch) {
          current = mergeAgentChatSnapshot(null, next)
          browsing = false
          setBrowsingOlder(false)
          setSnapshot(current)
          setError(null)
        }
      } catch (cause) {
        if (!disposed) setError(cause instanceof Error ? cause.message : String(cause))
      } finally {
        returningLatest = false
      }
    }
    latestRef.current = returnLatest
    refreshRef.current = refresh
    olderRef.current = loadOlder
    void refresh()
    schedule()
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
      if (timer !== undefined) clearTimeout(timer)
      unlisten?.()
    }
  }, [terminalId])

  const refresh = useCallback(() => refreshRef.current(), [])
  const loadOlder = useCallback(() => olderRef.current(), [])
  const returnLatest = useCallback(() => latestRef.current(), [])
  return {
    snapshot,
    error,
    absence,
    refresh,
    loadOlder,
    loadingOlder,
    browsingOlder,
    returnLatest
  }
}
