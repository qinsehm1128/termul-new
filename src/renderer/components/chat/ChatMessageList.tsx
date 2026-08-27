import { useVirtualizer } from '@tanstack/react-virtual'
import { useEffect, useMemo, useRef, useState } from 'react'
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
  useMessageScroller
} from '@/components/ui/message-scroller'
import { useRuntimeTranslation } from '@/i18n/use-runtime-translation'
import type { AgentId, SessionId } from '@/lib/acp-api'
import type { FilePathResolutionContext } from '@/lib/file-path-links'
import { cn } from '@/lib/utils'
import { useAcpStore } from '@/stores/acp-store'
import { ChatEmptyState } from './ChatEmptyState'
import { ChatMessage } from './ChatMessage'
import {
  CHAT_CONTENT_WIDTH,
  CHAT_GUTTER_X,
  CHAT_STREAM_PAD_Y,
  chatTimelineRowClass
} from './chat-layout'
import { groupTurnActivity, type TimelineItem, type TurnTimelineItem } from './chat-timeline'
import { ThoughtGroup } from './ThoughtGroup'
import { ToolCallCard } from './ToolCallCard'
import { TurnActivity } from './TurnActivity'

/** Compact top-of-thread status for older-message / history backfill. Overlay — no scroll jump. */
export function ChatHistoryLoadingStatus(): React.JSX.Element {
  const t = useRuntimeTranslation('chat')
  return (
    <div
      role="status"
      aria-live="polite"
      aria-atomic="true"
      data-testid="chat-history-loading"
      className={cn(CHAT_GUTTER_X, 'pointer-events-none')}
    >
      <div className={cn(CHAT_CONTENT_WIDTH, 'flex items-center gap-2 py-1.5')}>
        <span
          className="h-1.5 w-16 animate-pulse rounded-sm bg-muted motion-reduce:animate-none"
          aria-hidden="true"
        />
        <span
          className="h-1.5 w-28 animate-pulse rounded-sm bg-muted/70 motion-reduce:animate-none"
          aria-hidden="true"
        />
        <span className="text-2xs text-muted-foreground">
          {t('history.loadingOlder', 'Loading earlier messages…')}
        </span>
      </div>
    </div>
  )
}

/**
 * Reverse-infinite-scroll load of older messages. Returns a React flag for the
 * overlay so the effect can keep a ref guard without depending on render state.
 */
export function useLoadOlderMessages(
  sessionId: SessionId,
  itemCount: number,
  startIndex: number | undefined,
  viewportEl: HTMLDivElement | null,
  pinned: boolean
): boolean {
  const [loadingOlder, setLoadingOlder] = useState(false)
  const loadingOlderRef = useRef(false)

  useEffect(() => {
    if (startIndex === undefined || startIndex > 0) return
    if (itemCount === 0 || loadingOlderRef.current) return
    if (viewportEl === null || pinned) return
    if (viewportEl.scrollHeight <= viewportEl.clientHeight) return
    const prevScrollHeight = viewportEl.scrollHeight
    const prevScrollTop = viewportEl.scrollTop
    // Cancel on dependency change (e.g. session switch) so a load that resolves
    // after the reader moved to another chat never adjusts the new viewport.
    let cancelled = false
    loadingOlderRef.current = true
    setLoadingOlder(true)
    void useAcpStore
      .getState()
      .loadOlderMessages(sessionId, 50)
      .then(() => {
        if (cancelled) return
        // Restore the reader's position after older rows are prepended above.
        requestAnimationFrame(() => {
          if (cancelled || !viewportEl) return
          viewportEl.scrollTop = prevScrollTop + (viewportEl.scrollHeight - prevScrollHeight)
        })
      })
      .finally(() => {
        loadingOlderRef.current = false
        if (!cancelled) setLoadingOlder(false)
      })
    return () => {
      cancelled = true
      loadingOlderRef.current = false
      setLoadingOlder(false)
    }
  }, [startIndex, itemCount, sessionId, viewportEl, pinned])

  return loadingOlder
}

/** Reports the live item count to the scroller so the jump button can badge unread. */
function ItemCountReporter({ count }: { count: number }): null {
  const { setItemCount } = useMessageScroller()
  useEffect(() => {
    setItemCount(count)
  }, [count, setItemCount])
  return null
}

interface ChatMessageListProps {
  items: TimelineItem[]
  /** Active session — resets enter-animation baseline on switch. */
  sessionId: SessionId
  /** Agent behind this session (drives the agent name/icon on replies). */
  agentId: AgentId
  /** True for the complete duration of an in-flight agent turn. */
  showRunningIndicator: boolean
  /** Seed the composer with a user message's text (edit affordance). */
  onEditMessage?: (text: string) => void
  /** Re-run the latest user turn (regenerate affordance on agent replies). */
  onRetry?: () => void
  /** Filesystem roots used for safe file-path links in agent prose. */
  filePathContext?: FilePathResolutionContext
}

/** Index of the last visible message item in the turn-grouped timeline. */
function lastMessageIndex(items: TurnTimelineItem[]): number {
  for (let i = items.length - 1; i >= 0; i--) {
    if (items[i].kind === 'message') return i
  }
  return -1
}

/** Stable id for animate-enter tracking across message, tool, thought, and activity rows. */
function timelineItemId(it: TimelineItem): string {
  if (it.kind === 'message') return it.message.id
  if (it.kind === 'tool') return it.tool.toolCallId
  return it.key
}

/**
 * Returns true for timeline items that arrived after the list's first paint
 * (or after a session switch). History loaded on open does not re-enter.
 */
function useAnimateEnter(sessionId: SessionId, items: TimelineItem[]): (id: string) => boolean {
  const sessionRef = useRef(sessionId)
  const initialIdsRef = useRef<Set<string> | null>(null)

  useEffect(() => {
    if (sessionRef.current !== sessionId) {
      sessionRef.current = sessionId
      initialIdsRef.current = null
    }
  }, [sessionId])

  if (initialIdsRef.current === null) {
    initialIdsRef.current = new Set(items.map(timelineItemId))
  }

  return (id: string) => !initialIdsRef.current!.has(id)
}

/** Props shared between the list and its virtualized inner timeline. */
interface TimelineRenderProps {
  sessionId: SessionId
  groupedItems: TurnTimelineItem[]
  lastMsgIndex: number
  shouldAnimateEnter: (id: string) => boolean
  onEditMessage?: (text: string) => void
  onRetry?: () => void
  filePathContext?: FilePathResolutionContext
  onLoadingOlderChange?: (loading: boolean) => void
}

/**
 * Virtualized timeline body. Lives inside <MessageScrollerProvider> so it can
 * read `viewportEl` (the virtualizer's scroll element) and `pinned` (follow
 * state) from the scroller context. Only near-viewport rows are mounted.
 */
function VirtualizedTimeline({
  sessionId,
  groupedItems,
  lastMsgIndex,
  shouldAnimateEnter,
  onEditMessage,
  onRetry,
  filePathContext,
  onLoadingOlderChange
}: TimelineRenderProps): React.JSX.Element {
  const { viewportEl, pinned } = useMessageScroller()
  const virtualizer = useVirtualizer({
    count: groupedItems.length,
    getScrollElement: () => viewportEl,
    estimateSize: () => 120,
    overscan: 6,
    getItemKey: (i) => groupedItems[i]?.key ?? i
  })

  // Stick-to-bottom while streaming: only auto-follow when the reader is
  // pinned to the live edge (followOnAppend — do NOT pull a reader who has
  // scrolled up to read history back down).
  useEffect(() => {
    if (pinned && groupedItems.length > 0) {
      virtualizer.scrollToIndex(groupedItems.length - 1, { align: 'end' })
    }
  }, [groupedItems.length, pinned, virtualizer])

  // Reverse-infinite-scroll: lazy-load older messages ONLY on genuine reader
  // intent — the viewport must be scrollable and the reader must have scrolled
  // up off the live edge (not pinned). Without this gate, the first paint (and
  // any session short enough to fit the viewport) has startIndex === 0 and would
  // fire loadOlderMessages with no intent; each prepend changes groupedItems.length
  // and re-runs the effect, cascading until the whole transcript is back in the
  // live window and undoing the bound. The store guards concurrent loads and is
  // idempotent at the history head; this local flag avoids spamming on rapid
  // range notifications. The reader's position is preserved across the prepend.
  const startIndex = virtualizer.range?.startIndex
  const loadingOlder = useLoadOlderMessages(
    sessionId,
    groupedItems.length,
    startIndex,
    viewportEl,
    pinned
  )
  useEffect(() => {
    onLoadingOlderChange?.(loadingOlder)
  }, [loadingOlder, onLoadingOlderChange])

  // When the reader returns to the live edge, drop the per-session backfill
  // allowance so the next coalesced flush trims the window back to the live
  // bound. Bounded browsing: load-on-scroll-up grows the retained window; coming
  // back to the live edge shrinks it again. Best-effort (optional chaining) so
  // isolated tests that mock the store don't crash on the unconditional call.
  useEffect(() => {
    if (!pinned) return
    useAcpStore.getState?.()?.clearSessionBackfill?.(sessionId)
  }, [pinned, sessionId])

  const rowClass = (item: TurnTimelineItem): string =>
    chatTimelineRowClass(item.kind, item.kind === 'message' ? item.message.role : undefined)

  const renderItemContent = (item: TurnTimelineItem, index: number): React.JSX.Element => {
    if (item.kind === 'activity') {
      return (
        <TurnActivity
          items={item.items}
          active={item.active}
          durationMs={item.durationMs}
          attentionRequired={item.attentionRequired}
          hasFinalResponse={item.hasFinalResponse}
          shouldAnimateEnter={shouldAnimateEnter}
          filePathContext={filePathContext}
        />
      )
    }
    if (item.kind === 'tool') {
      return (
        <ToolCallCard
          toolCall={item.tool}
          animateEnter={shouldAnimateEnter(item.tool.toolCallId)}
          filePathContext={filePathContext}
        />
      )
    }
    if (item.kind === 'thought-group') {
      return <ThoughtGroup messages={item.messages} isLiveTail={false} />
    }
    return (
      <ChatMessage
        message={item.message}
        showHeader
        isLast={index === groupedItems.length - 1}
        isTurnTail={item.isTurnTail}
        turnText={item.turnText}
        actionsPinned={index === lastMsgIndex}
        animateEnter={item.isTurnTail ? false : shouldAnimateEnter(item.message.id)}
        onEdit={onEditMessage}
        onRetry={onRetry}
        filePathContext={filePathContext}
      />
    )
  }

  // Fallback: when the viewport can't be measured (zero height — jsdom in tests,
  // or the pre-measurement first paint), render all items in normal flow so
  // content is always present. A real production viewport yields virtual items
  // and the windowed path runs.
  const virtualItems = virtualizer.getVirtualItems()
  if (viewportEl === null || virtualItems.length === 0) {
    return (
      <MessageScrollerContent className={CHAT_CONTENT_WIDTH}>
        {groupedItems.map((item, index) => (
          <MessageScrollerItem
            key={item.key}
            messageId={item.key}
            className={rowClass(item)}
            data-timeline-kind={item.kind}
            scrollAnchor={item.kind === 'message' && item.message.role === 'user'}
          >
            {renderItemContent(item, index)}
          </MessageScrollerItem>
        ))}
      </MessageScrollerContent>
    )
  }

  return (
    <MessageScrollerContent
      className={CHAT_CONTENT_WIDTH}
      style={{ height: `${virtualizer.getTotalSize()}px`, position: 'relative' }}
    >
      {virtualItems.map((virtualItem) => {
        const item = groupedItems[virtualItem.index]
        if (!item) return null
        return (
          <MessageScrollerItem
            key={virtualItem.key}
            ref={virtualizer.measureElement}
            messageId={item.key}
            className={rowClass(item)}
            scrollAnchor={item.kind === 'message' && item.message.role === 'user'}
            data-index={virtualItem.index}
            data-timeline-kind={item.kind}
            style={{
              position: 'absolute',
              top: 0,
              left: 0,
              width: '100%',
              transform: `translateY(${virtualItem.start}px)`
            }}
          >
            {renderItemContent(item, virtualItem.index)}
          </MessageScrollerItem>
        )
      })}
    </MessageScrollerContent>
  )
}

/**
 * Scrollable message thread built on the MessageScroller engine. Agent process
 * output is grouped into one turn-level disclosure; the final reply remains a
 * normal message below it.
 */
export function ChatMessageList({
  items,
  sessionId,
  agentId,
  showRunningIndicator,
  onEditMessage,
  onRetry,
  filePathContext
}: ChatMessageListProps): React.JSX.Element {
  const groupedItems = useMemo(
    () => groupTurnActivity(items, showRunningIndicator),
    [items, showRunningIndicator]
  )
  const lastMsgIndex = useMemo(() => lastMessageIndex(groupedItems), [groupedItems])
  const shouldAnimateEnter = useAnimateEnter(sessionId, items)
  const [loadingOlder, setLoadingOlder] = useState(false)

  if (items.length === 0 && !showRunningIndicator) {
    return <ChatEmptyState agentId={agentId} onPick={onEditMessage} />
  }

  return (
    <div className="relative min-h-0 flex-1">
      {loadingOlder ? (
        <div className="pointer-events-none absolute inset-x-0 top-0 z-10">
          <ChatHistoryLoadingStatus />
        </div>
      ) : null}
      <MessageScrollerProvider autoScroll>
        <ItemCountReporter count={groupedItems.length} />
        <MessageScroller>
          <MessageScrollerViewport className={cn(CHAT_GUTTER_X, CHAT_STREAM_PAD_Y)}>
            <VirtualizedTimeline
              sessionId={sessionId}
              groupedItems={groupedItems}
              lastMsgIndex={lastMsgIndex}
              shouldAnimateEnter={shouldAnimateEnter}
              filePathContext={filePathContext}
              onEditMessage={onEditMessage}
              onRetry={onRetry}
              onLoadingOlderChange={setLoadingOlder}
            />
          </MessageScrollerViewport>
          <MessageScrollerButton />
        </MessageScroller>
      </MessageScrollerProvider>
    </div>
  )
}
