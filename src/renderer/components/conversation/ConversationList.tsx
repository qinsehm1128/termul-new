import { AlertTriangle, ChevronDown } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { AgentIcon } from '@/components/agents/AgentIcon'
import { ConversationLifecycleActions } from '@/components/chat/ChatHistoryEntryRow'
import {
  ListEmptyState,
  ListLoadingState,
  ListRow,
  ListRowMeta,
  ListRowStatus
} from '@/components/lists'
import { pathBasename } from '@/components/lists/path-basename'
import { getAgentById } from '@/lib/agents/custom-agents'
import { agentIdForConversation } from '@/lib/conversation-list-meta'
import { conversationRowStatus } from '@/lib/conversation-row-status'
import { displayConversationTitle, sessionTitleForConversation } from '@/lib/conversation-title'
import { formatRelativeTime } from '@/lib/git-time'
import { cn } from '@/lib/utils'
import { useAcpStore } from '@/stores/acp-store'
import {
  recoveryCountForConversation,
  useConversationStore,
  useVisibleConversations
} from '@/stores/conversation-store'
import { useProjectStore } from '@/stores/project-store'

const DEFAULT_PAGE_SIZE = 50

interface ConversationListProps {
  projectId?: string
  pageSize?: number
  onConversationOpened?: () => void
  className?: string
}

export function ConversationList({
  projectId,
  pageSize = DEFAULT_PAGE_SIZE,
  onConversationOpened,
  className
}: ConversationListProps): React.JSX.Element {
  const { t } = useTranslation('common')
  const { t: tConversation } = useTranslation('conversation')
  const navigate = useNavigate()
  const conversations = useVisibleConversations()
  const recoveryItems = useConversationStore((state) => state.recoveryItems)
  const activeConversationId = useConversationStore((state) => state.activeConversationId)
  const loadingList = useConversationStore((state) => state.loadingList)
  const listError = useConversationStore((state) => state.listError)
  const sessions = useAcpStore((state) => state.sessions)
  const sessionIndex = useAcpStore((state) => state.sessionIndex)
  const pendingPermissions = useAcpStore((state) => state.pendingPermissions)
  const pendingQuestions = useAcpStore((state) => state.pendingQuestions)
  const projects = useProjectStore((state) => state.projects)
  const [visibleCount, setVisibleCount] = useState(pageSize)
  const [expandedId, setExpandedId] = useState<string | null>(null)

  const projected = useMemo(
    () =>
      projectId
        ? conversations.filter(
            (conversation) => conversation.projectAttachment?.projectId === projectId
          )
        : conversations,
    [conversations, projectId]
  )
  const visible = projected.slice(0, visibleCount)
  const hasMore = visible.length < projected.length

  if (projected.length === 0) {
    if (listError) {
      return (
        <ListEmptyState
          tone="error"
          title={t('conversationRoute.errors.title')}
          message={listError.message}
        />
      )
    }

    if (loadingList) {
      return <ListLoadingState label={tConversation('dashboard.loading')} />
    }

    return <ListEmptyState message={t('conversationNavigation.empty')} />
  }

  return (
    <div className={cn('flex flex-col py-1', className)} data-testid="conversation-list">
      {visible.map((conversation) => {
        const title = displayConversationTitle(conversation, {
          sessionTitle: sessionTitleForConversation(
            conversation.conversationId,
            sessions,
            sessionIndex
          ),
          untitled: t('conversationNavigation.untitled')
        })
        const project = conversation.projectAttachment
          ? projects.find((candidate) => candidate.id === conversation.projectAttachment?.projectId)
          : undefined
        const projectLabel = conversation.projectAttachment
          ? project?.name || conversation.projectAttachment.projectPathSnapshot
          : t('conversationNavigation.projectless')
        const recoveryCount = recoveryCountForConversation(
          recoveryItems,
          conversation.conversationId
        )
        const isActive = activeConversationId === conversation.conversationId
        const folder = pathBasename(conversation.workspaceCwd)
        const preview =
          folder && folder !== title ? <span className="font-mono">{folder}</span> : undefined
        const agentId = agentIdForConversation(conversation.conversationId, sessions, sessionIndex)
        const agentLabel = agentId ? (getAgentById(agentId)?.name ?? agentId) : undefined
        const rowStatus = conversationRowStatus(
          conversation.conversationId,
          sessions,
          sessionIndex,
          pendingPermissions,
          pendingQuestions
        )
        const expanded = expandedId === conversation.conversationId
        return (
          <div key={conversation.conversationId} data-conversation-id={conversation.conversationId}>
            <ListRow
              density="compact"
              active={isActive}
              title={
                <span className="flex min-w-0 items-center gap-1.5">
                  {agentId ? <AgentIcon agentId={agentId} className="size-3.5" /> : null}
                  <span className="min-w-0 truncate">{title}</span>
                  <ListRowStatus
                    status={rowStatus}
                    label={t(`conversationNavigation.rowStatus.${rowStatus}`)}
                  />
                </span>
              }
              titleAttr={title}
              preview={preview}
              meta={
                <ListRowMeta
                  items={[
                    formatRelativeTime(conversation.createdAtUtc),
                    agentLabel,
                    conversation.lastSeq > 0
                      ? t('conversationNavigation.revision', { count: conversation.lastSeq })
                      : null,
                    recoveryCount > 0 ? (
                      <span
                        key="recovery"
                        role="status"
                        className="inline-flex items-center gap-0.5 text-destructive"
                        aria-label={t('conversationNavigation.recoveryBadge', {
                          count: recoveryCount
                        })}
                      >
                        <AlertTriangle className="size-3" aria-hidden="true" />
                        {recoveryCount}
                      </span>
                    ) : null
                  ]}
                />
              }
              trailing={
                <>
                  <button
                    type="button"
                    className="inline-flex size-7 items-center justify-center rounded-md hover:bg-sidebar-accent hover:text-foreground"
                    aria-expanded={expanded}
                    aria-label={t('conversationNavigation.toggleDetails')}
                    onClick={(event) => {
                      event.stopPropagation()
                      setExpandedId((current) =>
                        current === conversation.conversationId ? null : conversation.conversationId
                      )
                    }}
                  >
                    <ChevronDown
                      className={cn('size-3.5 transition-transform', expanded && 'rotate-180')}
                    />
                  </button>
                  <ConversationLifecycleActions
                    conversationId={conversation.conversationId}
                    title={title}
                    className="size-7"
                  />
                </>
              }
              details={
                <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-2 gap-y-1">
                  <dt>{t('conversationNavigation.detailsWorkspace')}</dt>
                  <dd className="truncate" title={conversation.workspaceCwd}>
                    {conversation.workspaceCwd}
                  </dd>
                  <dt>{t('conversationNavigation.detailsProject')}</dt>
                  <dd className="truncate">{projectLabel}</dd>
                  <dt>{t('conversationNavigation.detailsState')}</dt>
                  <dd>{conversation.lifecycleState}</dd>
                </dl>
              }
              expanded={expanded}
              onClick={() => {
                navigate(`/c/${conversation.conversationId}`)
                onConversationOpened?.()
              }}
            />
          </div>
        )
      })}
      {hasMore && (
        <button
          type="button"
          className="mx-1 mt-0.5 inline-flex h-8 items-center justify-center rounded-md px-2 text-xs text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          onClick={() => setVisibleCount((count) => count + pageSize)}
        >
          {t('conversationNavigation.loadMore', { count: projected.length - visible.length })}
        </button>
      )}
    </div>
  )
}
