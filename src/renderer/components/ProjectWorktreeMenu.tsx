import { Check, FolderGit2, GitBranch, Plus } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { reconcileProjectWorktreesNow } from '@/hooks/use-projects-persistence'
import { cn } from '@/lib/utils'
import type { Project } from '@/types/project'

interface ProjectWorktreeMenuProps {
  project: Project
  /** `worktreeId` is null for the project's own checkout. */
  onOpenWorktreeTerminal: (projectId: string, worktreeId: string | null) => void
  onCreateWorktree: (projectId: string) => void
}

function samePath(a: string, b: string): boolean {
  const normalize = (path: string) => path.replace(/\\/g, '/').replace(/\/+$/, '')
  return normalize(a) === normalize(b)
}

// The menu lives in a portal, but React still bubbles its events through the
// component tree: without this a pick would also click, key or right-click the
// project row underneath.
const stopRowEvent = (e: React.SyntheticEvent): void => {
  e.stopPropagation()
}

/**
 * The project row's worktree button: lists the checkouts git knows about and
 * opens a terminal in the one picked, or starts the new-worktree flow.
 */
export function ProjectWorktreeMenu({
  project,
  onOpenWorktreeTerminal,
  onCreateWorktree
}: ProjectWorktreeMenuProps): React.JSX.Element {
  const { t } = useTranslation('projects')
  const projectPath = project.path ?? ''
  // `git worktree list` names the main checkout too; it is the root entry here.
  const mainCheckout = project.worktrees?.find((w) => samePath(w.path, projectPath))
  const linked = (project.worktrees ?? []).filter((w) => !samePath(w.path, projectPath))
  const activeLinkedId = linked.some((w) => w.id === project.activeWorktreeId)
    ? project.activeWorktreeId
    : null

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        // Opening is the moment to look again: worktrees added from a shell
        // since the last pass should be in the list the user is reading.
        if (open) void reconcileProjectWorktreesNow(project.id)
      }}
    >
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          onClick={stopRowEvent}
          onKeyDown={stopRowEvent}
          className={cn(
            'mr-1 inline-flex size-4 shrink-0 items-center justify-center rounded-sm transition-colors hover:bg-sidebar-accent hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring data-[state=open]:bg-sidebar-accent data-[state=open]:text-foreground',
            linked.length > 0 ? 'text-sidebar-foreground' : 'text-muted-foreground/60'
          )}
          title={t('worktreeMenu.title')}
          aria-label={t('worktreeMenu.openFor', { name: project.name })}
          data-testid={`project-worktree-menu-${project.id}`}
        >
          <GitBranch size={11} />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="w-64"
        onClick={stopRowEvent}
        onKeyDown={stopRowEvent}
        onContextMenu={stopRowEvent}
      >
        <DropdownMenuLabel>{t('worktreeMenu.title')}</DropdownMenuLabel>
        <DropdownMenuItem onSelect={() => onOpenWorktreeTerminal(project.id, null)}>
          <FolderGit2 className="mr-2 h-4 w-4 shrink-0" />
          <span className="min-w-0 flex-1 truncate">{t('worktreeMenu.root')}</span>
          {mainCheckout && (
            <span className="ml-2 max-w-24 truncate text-xs text-muted-foreground">
              {mainCheckout.branch}
            </span>
          )}
          <Check className={cn('ml-2 h-3.5 w-3.5 shrink-0', activeLinkedId && 'invisible')} />
        </DropdownMenuItem>
        {linked.map((worktree) => (
          <DropdownMenuItem
            key={worktree.id}
            title={worktree.path}
            onSelect={() => onOpenWorktreeTerminal(project.id, worktree.id)}
          >
            <GitBranch className="mr-2 h-4 w-4 shrink-0" />
            <span className="min-w-0 flex-1 truncate">{worktree.name}</span>
            {worktree.branch !== worktree.name && (
              <span className="ml-2 max-w-24 truncate text-xs text-muted-foreground">
                {worktree.branch}
              </span>
            )}
            <Check
              className={cn(
                'ml-2 h-3.5 w-3.5 shrink-0',
                worktree.id !== activeLinkedId && 'invisible'
              )}
            />
          </DropdownMenuItem>
        ))}
        {linked.length === 0 && (
          <p className="px-2 py-1.5 text-xs text-muted-foreground">{t('worktreeMenu.empty')}</p>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => onCreateWorktree(project.id)}>
          <Plus className="mr-2 h-4 w-4" /> {t('worktreeMenu.create')}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
