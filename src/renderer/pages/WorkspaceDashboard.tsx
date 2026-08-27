import { MessageSquarePlus } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { ConversationList } from '@/components/conversation/ConversationList'
import { useConversationStore } from '@/stores/conversation-store'
import { useWorkspaceStore } from '@/stores/workspace-store'

export default function WorkspaceDashboard(): React.JSX.Element {
  const { t } = useTranslation('conversation')
  const { t: tCommon } = useTranslation('common')
  const loading = useConversationStore((state) => state.loadingList)
  const listError = useConversationStore((state) => state.listError)
  const conversationIds = useConversationStore((state) => state.conversationIds)
  const activeConversationId = useConversationStore((state) => state.activeConversationId)
  const activeConversation = useConversationStore((state) =>
    activeConversationId ? state.summariesById[activeConversationId] : undefined
  )

  return (
    <section
      className="flex h-full min-h-0 flex-col bg-background"
      aria-labelledby="conversation-dashboard-title"
    >
      <div className="mx-auto flex w-full max-w-2xl min-h-0 flex-1 flex-col px-4">
        <header className="shrink-0 border-b border-border/70 py-5">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
            <div className="min-w-0">
              <h1
                id="conversation-dashboard-title"
                className="text-lg font-semibold tracking-tight text-foreground"
              >
                {t('dashboard.title')}
              </h1>
              <p className="mt-1 max-w-prose text-sm leading-relaxed text-muted-foreground">
                {t('dashboard.description')}
              </p>
              {activeConversation ? (
                <p className="mt-2 text-2xs text-muted-foreground">
                  <span className="font-medium text-foreground/80">
                    {t('dashboard.workspace')}:
                  </span>{' '}
                  <code className="break-all font-mono">{activeConversation.workspaceCwd}</code>
                </p>
              ) : null}
            </div>
            <button
              type="button"
              className="inline-flex h-8 shrink-0 items-center gap-1.5 self-start rounded-md bg-primary px-2.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              onClick={() => {
                const paneId = useWorkspaceStore.getState().activePaneId
                if (paneId) useWorkspaceStore.getState().showAgentLauncher(paneId)
              }}
            >
              <MessageSquarePlus className="size-3.5" aria-hidden="true" />
              {t('navigation.newChat')}
            </button>
          </div>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto py-2">
          {listError && conversationIds.length === 0 ? (
            <div className="px-2 py-6" role="alert">
              <p className="text-sm font-medium text-foreground">
                {tCommon('conversationRoute.errors.title')}
              </p>
              <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
                {listError.message}
              </p>
            </div>
          ) : loading ? (
            <div role="status" aria-busy="true" className="flex flex-col gap-1 px-1 py-2">
              <p className="px-2 pb-1 text-xs text-muted-foreground">{t('dashboard.loading')}</p>
              {Array.from({ length: 6 }, (_, index) => (
                <div key={index} className="flex h-8 items-center px-2">
                  <span className="h-2.5 w-[42%] animate-pulse rounded-sm bg-muted" />
                </div>
              ))}
            </div>
          ) : conversationIds.length === 0 ? (
            <div className="px-2 py-6">
              <p className="max-w-prose text-sm leading-relaxed text-muted-foreground">
                {t('dashboard.empty')}
              </p>
            </div>
          ) : (
            <ConversationList />
          )}
        </div>
      </div>
    </section>
  )
}
