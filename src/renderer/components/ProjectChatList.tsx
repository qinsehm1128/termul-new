import { Search, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { ConversationList } from '@/components/conversation/ConversationList'
import { useConversationStore } from '@/stores/conversation-store'

interface ProjectChatListProps {
  projectId: string
}

/** Optional project projection over the canonical global ConversationStore list. */
export function ProjectChatList({ projectId }: ProjectChatListProps): React.JSX.Element {
  const { t } = useTranslation('common')
  const searchQuery = useConversationStore((state) => state.searchQuery)
  const setSearchQuery = useConversationStore((state) => state.setSearchQuery)

  return (
    <section className="flex min-h-0 flex-col" aria-label={t('conversationNavigation.projectView')}>
      <div className="relative border-b border-sidebar-border p-2">
        <Search
          className="pointer-events-none absolute left-4 top-1/2 size-3 -translate-y-1/2 text-muted-foreground"
          aria-hidden="true"
        />
        <input
          type="search"
          value={searchQuery}
          onChange={(event) => setSearchQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Escape') setSearchQuery('')
          }}
          placeholder={t('conversationNavigation.search')}
          aria-label={t('conversationNavigation.search')}
          className="h-8 w-full rounded-md bg-background pl-7 pr-7 text-xs outline-none focus-visible:ring-2 focus-visible:ring-primary"
        />
        {searchQuery && (
          <button
            type="button"
            className="absolute right-3 top-1/2 inline-flex size-6 -translate-y-1/2 items-center justify-center text-muted-foreground"
            aria-label={t('conversationNavigation.clearSearch')}
            onClick={() => setSearchQuery('')}
          >
            <X className="size-3" aria-hidden="true" />
          </button>
        )}
      </div>
      <div className="max-h-80 overflow-y-auto">
        <ConversationList projectId={projectId} pageSize={10} />
      </div>
    </section>
  )
}
