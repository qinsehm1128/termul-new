import type { ConversationId } from '@shared/types/conversation.types'
import { useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import { ConversationRecoveryPanel } from '@/components/conversation/ConversationRecoveryPanel'
import { Button } from '@/components/ui/button'
import { resolveSessionWorkspaceConflict } from '@/hooks/use-session-workspace-sync'
import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'

export interface WorkspaceConflictBannerProps {
  conversationId?: ConversationId | null
}

export function WorkspaceConflictBanner({
  conversationId: conversationIdProp
}: WorkspaceConflictBannerProps = {}): null | React.ReactElement {
  const { t } = useTranslation('workspace')
  const storeConversationId = useSessionWorkspaceSyncStore((state) => state.activeConversationId)
  const conversationId = conversationIdProp ?? storeConversationId
  const conflict = useSessionWorkspaceSyncStore((state) =>
    conversationId ? state.conflictsByConversation[conversationId] : undefined
  )
  const recoveryItemsValue = useSessionWorkspaceSyncStore((state) =>
    conversationId ? state.recoveryByConversation[conversationId] : undefined
  )
  const recoveryItems = recoveryItemsValue ?? []
  const setRecoveryItems = useSessionWorkspaceSyncStore((state) => state.setRecoveryItems)

  const resolveConflict = useCallback(
    (action: 'reload' | 'overwrite' | 'dismiss') => {
      if (conversationId) void resolveSessionWorkspaceConflict(conversationId, action)
    },
    [conversationId]
  )

  if (!conversationId || (!conflict && recoveryItems.length === 0)) return null

  return (
    <div
      role="alert"
      aria-live="polite"
      data-conversation-id={conversationId}
      className="flex max-h-[45vh] flex-col gap-3 overflow-auto border-b border-amber-500/50 bg-amber-500/10 px-3 py-2"
    >
      {conflict ? (
        <section className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
          <div className="min-w-0">
            <div className="text-sm font-medium text-amber-700 dark:text-amber-300">
              {t('conflict.title')}
            </div>
            <div className="break-words text-xs text-muted-foreground">
              Conversation {conversationId} · revision {conflict.currentRevision}
              {conflict.currentUpdatedAtUtc ? ` · ${conflict.currentUpdatedAtUtc}` : ''}
              {conflict.currentUpdateIdentity ? ` · ${conflict.currentUpdateIdentity}` : ''}
            </div>
          </div>
          <div className="flex shrink-0 flex-wrap gap-2">
            <Button
              type="button"
              variant="default"
              size="xs"
              onClick={() => resolveConflict('reload')}
            >
              {t('conflict.reload')}
            </Button>
            <Button
              type="button"
              variant="secondary"
              size="xs"
              onClick={() => resolveConflict('overwrite')}
            >
              {t('conflict.overwrite')}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="xs"
              onClick={() => resolveConflict('dismiss')}
            >
              {t('conflict.dismiss')}
            </Button>
          </div>
        </section>
      ) : null}
      {recoveryItems.length > 0 ? (
        <ConversationRecoveryPanel
          embedded
          items={recoveryItems}
          conversationId={conversationId}
          className="max-h-none border-0 bg-transparent p-0 shadow-none backdrop-blur-none"
          onItemsChange={(updated) => setRecoveryItems(conversationId, updated)}
        />
      ) : null}
    </div>
  )
}
