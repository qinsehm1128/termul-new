import type { AgentChatControlAction } from '@shared/types/agent-chat.types'
import { type ReactNode, useMemo, useRef, useState } from 'react'
import { ChatMessageList } from '../../features/agent-session/chat/ChatMessageList'
import { buildTimeline } from '../../features/agent-session/chat/chat-timeline'
import { useTerminalAgentChat } from '../../hooks/use-terminal-agent-chat'
import { agentChatApi } from '../../lib/agent-chat-api'
import { logFrontendError } from '../../lib/log-api'
import { useTerminalStore } from '../../stores/terminal-store'

type View = 'terminal' | 'chat'
const storagePrefix = 'se:terminal-agent-view:'
const views = new Map<string, View>()

function rememberedView(terminalId: string): View {
  const cached = views.get(terminalId)
  if (cached) return cached
  try {
    return localStorage.getItem(`${storagePrefix}${terminalId}`) === 'chat' ? 'chat' : 'terminal'
  } catch {
    return 'terminal'
  }
}

const deliveryLabels = {
  queued: '排队中',
  pending: '等待确认',
  delivered: '已送达',
  unconfirmed: '未确认'
} as const

function TerminalAgentChat({ terminalId }: { terminalId: string }) {
  const { snapshot, error, refresh, loadOlder, loadingOlder, browsingOlder, returnLatest } =
    useTerminalAgentChat(terminalId)
  const tracked = useTerminalStore(
    (state) => state.terminals.find((terminal) => terminal.ptyId === terminalId)?.agentSession
  )
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const sending = useRef(false)
  const timeline = useMemo(
    () => buildTimeline(snapshot?.messages ?? [], snapshot?.tool_calls ?? []),
    [snapshot?.messages, snapshot?.tool_calls]
  )
  const ready = snapshot?.state === 'ready' && !error

  const send = async () => {
    if (sending.current || !text.trim() || !ready) return
    sending.current = true
    setBusy(true)
    setActionError(null)
    try {
      // One request only: the host owns queuing and transcript-based confirmation.
      await agentChatApi.send(terminalId, text)
      setText('')
      await refresh()
    } catch (cause) {
      setActionError(`发送状态未知，请先在终端核实，不要重复发送。${String(cause)}`)
      void logFrontendError({
        source: 'terminal-agent-chat:send',
        level: 'warn',
        message: 'Terminal Agent send failed or unconfirmed; no automatic retry.'
      })
    } finally {
      sending.current = false
      setBusy(false)
    }
  }
  const forceAllowed =
    snapshot?.can_force_send === true &&
    !snapshot.deliveries.some((receipt) => receipt.status === 'pending') &&
    !['busy', 'running', 'compacting', 'waiting_permission', 'permission_request'].includes(
      snapshot.hook_state ?? ''
    )

  const forceSend = async (deliveryId: string) => {
    if (
      sending.current ||
      !forceAllowed ||
      !snapshot?.deliveries.some(
        (receipt) => receipt.id === deliveryId && receipt.status === 'queued'
      )
    )
      return
    sending.current = true
    setBusy(true)
    setActionError(null)
    try {
      await agentChatApi.send(terminalId, '', { force: true, delivery_id: deliveryId })
      await refresh()
    } catch (cause) {
      setActionError(`发送状态未知，请先在终端核实，不要重复发送。${String(cause)}`)
    } finally {
      sending.current = false
      setBusy(false)
    }
  }

  const control = async (action: AgentChatControlAction) => {
    if (sending.current) return
    sending.current = true
    setBusy(true)
    setActionError(null)
    try {
      await agentChatApi.control(terminalId, action)
      await refresh()
    } catch (cause) {
      setActionError(`操作失败：${String(cause)}`)
      void logFrontendError({
        source: 'terminal-agent-chat:control',
        level: 'warn',
        message: `Terminal Agent ${action} control failed; terminal fallback remains available.`
      })
    } finally {
      sending.current = false
      setBusy(false)
    }
  }

  return (
    <section className="flex h-full min-h-0 flex-col" aria-label="终端 Agent 对话">
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2 text-xs text-muted-foreground">
        <span>{snapshot?.agent ?? tracked?.agentId ?? 'Agent'}</span>
        <span>{snapshot?.hook_state || tracked?.hook_kind || '等待会话'}</span>
        {tracked?.source === 'hook' && <span>Hook</span>}
        <span className="ml-auto">审批请在终端中处理</span>
      </div>
      {(error || snapshot?.state === 'degraded') && (
        <div role="status" className="border-b border-border px-3 py-2 text-sm text-amber-500">
          对话视图已降级：{error || snapshot?.reason || '无法解析会话记录'}。请切回终端继续操作。
        </div>
      )}
      {!snapshot && !error && (
        <p role="status" className="p-3 text-sm text-muted-foreground">
          正在读取会话记录…
        </p>
      )}
      {snapshot?.state === 'ready' && timeline.length === 0 && (
        <p className="p-3 text-sm text-muted-foreground">暂无会话消息</p>
      )}
      {browsingOlder && (
        <button
          type="button"
          className="shrink-0 border-b border-border px-3 py-2 text-sm"
          onClick={() => void returnLatest()}
        >
          返回最新消息
        </button>
      )}
      {snapshot?.older_cursor != null && (
        <button
          type="button"
          className="shrink-0 border-b border-border px-3 py-2 text-sm disabled:opacity-50"
          disabled={loadingOlder}
          onClick={() => void loadOlder()}
        >
          {loadingOlder ? '正在加载…' : '加载更早消息'}
        </button>
      )}
      <ChatMessageList
        items={timeline}
        sessionId={snapshot?.session_id ?? `terminal:${terminalId}`}
        agentId={tracked?.agentId ?? snapshot?.agent ?? 'terminal-agent'}
        showRunningIndicator={false}
      />
      {snapshot && snapshot.deliveries.length > 0 && (
        <ul
          aria-label="消息送达状态"
          className="max-h-28 shrink-0 overflow-y-auto border-t border-border px-3 py-2 text-xs"
        >
          {snapshot.deliveries.map((delivery) => (
            <li key={delivery.id} className="flex gap-2 py-1">
              <span className="shrink-0 rounded bg-muted px-1.5" data-status={delivery.status}>
                {deliveryLabels[delivery.status]}
              </span>
              <span className="truncate">{delivery.text}</span>
              {delivery.status === 'queued' && forceAllowed && (
                <button
                  type="button"
                  className="shrink-0 rounded border border-border px-2 py-1"
                  disabled={busy}
                  onClick={() => void forceSend(delivery.id)}
                >
                  立即发送
                </button>
              )}
              {delivery.status === 'unconfirmed' && (
                <span className="shrink-0 text-muted-foreground">不会自动重发</span>
              )}
            </li>
          ))}
        </ul>
      )}
      <form
        className="shrink-0 space-y-2 border-t border-border p-3"
        onSubmit={(event) => {
          event.preventDefault()
          void send()
        }}
      >
        {actionError && (
          <p role="alert" className="text-xs text-destructive">
            {actionError}
          </p>
        )}
        <textarea
          aria-label="发送给终端 Agent"
          placeholder="发送给 Agent（忙碌时自动排队）"
          className="w-full resize-y rounded border border-border bg-background p-2 text-sm"
          rows={2}
          value={text}
          onChange={(event) => setText(event.target.value)}
          disabled={!ready || busy}
        />
        <div className="flex items-center justify-end gap-2">
          <button
            type="button"
            className="rounded border border-border px-2 py-1 text-xs"
            disabled={!snapshot || busy}
            onClick={() => void control('interrupt')}
          >
            中断 (Esc)
          </button>
          <button
            type="button"
            className="rounded border border-border px-2 py-1 text-xs"
            disabled={!snapshot || busy}
            onClick={() => void control('stop')}
          >
            强制停止 (Ctrl-C)
          </button>
          <button
            type="submit"
            className="rounded bg-primary px-3 py-1 text-xs text-primary-foreground disabled:opacity-50"
            disabled={!ready || busy || !text.trim()}
          >
            发送
          </button>
        </div>
      </form>
    </section>
  )
}

/** The PTY subtree stays mounted at the same position when switching views. */
export function TerminalAgentSurface({
  terminalId,
  ptyId,
  isVisible,
  children
}: {
  terminalId: string
  ptyId?: string
  isVisible: boolean
  children: (terminalVisible: boolean) => ReactNode
}) {
  const [view, setView] = useState<View>(() => rememberedView(terminalId))
  const selectView = (next: View) => {
    views.set(terminalId, next)
    try {
      localStorage.setItem(`${storagePrefix}${terminalId}`, next)
    } catch {
      /* Memory fallback. */
    }
    setView(next)
  }
  return (
    <div className="flex h-full min-h-0 flex-col">
      <fieldset className="flex shrink-0 gap-1 border-b border-border px-2 py-1">
        <legend className="sr-only">终端视图</legend>
        <button
          type="button"
          aria-pressed={view === 'terminal'}
          className="rounded px-2 py-0.5 text-xs aria-pressed:bg-muted"
          onClick={() => selectView('terminal')}
        >
          终端
        </button>
        <button
          type="button"
          aria-pressed={view === 'chat'}
          className="rounded px-2 py-0.5 text-xs aria-pressed:bg-muted"
          onClick={() => selectView('chat')}
        >
          对话
        </button>
      </fieldset>
      <div className="min-h-0 flex-1" hidden={view !== 'terminal'}>
        {children(isVisible && view === 'terminal')}
      </div>
      {view === 'chat' && (
        <div className="min-h-0 flex-1">
          {ptyId ? (
            <TerminalAgentChat key={ptyId} terminalId={ptyId} />
          ) : (
            <p role="status" className="p-3 text-sm text-muted-foreground">
              终端尚未就绪，请切回终端启动会话。
            </p>
          )}
        </div>
      )}
    </div>
  )
}
