import { MessageSquarePlus } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { ConversationList } from '@/components/conversation/ConversationList'
import { ListPanelHeader } from '@/components/lists'
import { useConversationStore, useVisibleConversations } from '@/stores/conversation-store'
import { useProjectStore } from '@/stores/project-store'

interface ConversationSidebarProps {
  onNewChat?: () => void
}

export function ConversationSidebar({ onNewChat }: ConversationSidebarProps): React.JSX.Element {
  const { t } = useTranslation('common')
  const searchQuery = useConversationStore((state) => state.searchQuery)
  const projectFilter = useConversationStore((state) => state.projectFilter)
  const setSearchQuery = useConversationStore((state) => state.setSearchQuery)
  const setProjectFilter = useConversationStore((state) => state.setProjectFilter)
  const projects = useProjectStore((state) => state.projects)
  const visibleCount = useVisibleConversations().length

  return (
    <aside className="flex h-full w-full min-w-0 flex-col bg-sidebar">
      <ListPanelHeader
        title={t('conversationNavigation.title')}
        shown={visibleCount}
        total={visibleCount}
        countLabel={t('conversationNavigation.shownCount', {
          shown: visibleCount,
          total: visibleCount
        })}
        search={searchQuery}
        onSearchChange={setSearchQuery}
        searchLabel={t('conversationNavigation.search')}
        clearSearchLabel={t('conversationNavigation.clearSearch')}
        actions={
          <button
            type="button"
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            aria-label={t('conversationNavigation.newChat')}
            title={t('conversationNavigation.newChat')}
            onClick={onNewChat}
          >
            <MessageSquarePlus className="size-3.5" aria-hidden="true" />
          </button>
        }
      >
        <label className="block">
          <span className="sr-only">{t('conversationNavigation.filter')}</span>
          <select
            value={projectFilter ?? 'all'}
            onChange={(event) =>
              setProjectFilter(event.target.value === 'all' ? null : event.target.value)
            }
            aria-label={t('conversationNavigation.filter')}
            className="h-7 w-full rounded-md border-0 bg-secondary/35 px-2 text-2xs text-foreground outline-none focus-visible:ring-1 focus-visible:ring-ring/50"
          >
            <option value="all">{t('conversationNavigation.allProjects')}</option>
            <option value="projectless">{t('conversationNavigation.projectless')}</option>
            {projects
              .filter((project) => !project.isArchived)
              .map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
          </select>
        </label>
      </ListPanelHeader>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <ConversationList />
      </div>
    </aside>
  )
}
