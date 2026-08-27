import { Search, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { ConversationList } from '@/components/conversation/ConversationList'
import { useConversationStore } from '@/stores/conversation-store'

/** Mobile/global Conversation navigation keyed only by canonical ConversationId. */
export function ChatHistoryTab({
  onSessionOpened
}: {
  onSessionOpened?: () => void
} = {}): React.JSX.Element {
  const { t } = useTranslation('common')
  const searchQuery = useConversationStore((state) => state.searchQuery)
  const setSearchQuery = useConversationStore((state) => state.setSearchQuery)

  return (
    <div className="flex h-full flex-col">
      <div className="relative border-b border-sidebar-border p-2">
        <Search
          className="pointer-events-none absolute left-4 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
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
          className="h-9 w-full rounded-md bg-background pl-8 pr-8 text-xs outline-none focus-visible:ring-2 focus-visible:ring-primary"
        />
        {searchQuery && (
          <button
            type="button"
            className="absolute right-3 top-1/2 inline-flex size-7 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground"
            aria-label={t('conversationNavigation.clearSearch')}
            onClick={() => setSearchQuery('')}
          >
            <X className="size-3" aria-hidden="true" />
          </button>
        )}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <ConversationList onConversationOpened={onSessionOpened} />
      </div>
    </div>
  )
}
