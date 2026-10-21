import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList
} from '@/components/ui/command'
import type { TerminalBoardStatusKey } from '@/lib/terminal-board'
import { terminalBoardStatus } from '@/lib/terminal-board'
import { openBoardTerminal } from '@/lib/terminal-board-navigation'
import {
  availableScopes,
  orderByRecency,
  scopeTerminals,
  type TerminalSwitcherContext,
  type TerminalSwitcherScope
} from '@/lib/terminal-switcher'
import { cn } from '@/lib/utils'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { isQuickTerminal, type Terminal } from '@/types/project'

const STATUS_DOT: Record<TerminalBoardStatusKey, string> = {
  live: 'bg-emerald-500',
  hidden: 'bg-muted-foreground/60',
  exited: 'bg-muted-foreground/60',
  disconnected: 'bg-destructive',
  attention: 'bg-amber-500'
}

interface TerminalQuickSwitcherProps {
  isOpen: boolean
  onClose: () => void
}

/**
 * Search-and-jump overlay for terminals.
 *
 * The switcher row above the panes is bounded by design; past roughly seven
 * entries a horizontal strip stops being readable. This is the unbounded
 * counterpart, and it costs no permanent screen space.
 *
 * Entries are recency-ordered, so opening it and pressing Enter reproduces the
 * last-terminal jump without typing. Filtering is left to cmdk so the match
 * behaviour is the same as the command palette's.
 */
export function TerminalQuickSwitcher({
  isOpen,
  onClose
}: TerminalQuickSwitcherProps): React.JSX.Element {
  const { t } = useTranslation('terminal')
  const navigate = useNavigate()

  const terminals = useTerminalStore((state) => state.terminals)
  const recentTerminalIds = useTerminalStore((state) => state.recentTerminalIds)
  const activeTerminalId = useTerminalStore((state) => state.activeTerminalId)
  const projects = useProjectStore((state) => state.projects)
  const activeProjectId = useProjectStore((state) => state.activeProjectId)
  const activeGroupId = useProjectStore((state) => state.activeGroupId)
  const groups = useProjectStore((state) => state.groups)

  const activeGroup = useMemo(
    () => groups.find((group) => group.id === activeGroupId) ?? null,
    [groups, activeGroupId]
  )

  // Opens on the widest scope: someone reaching for search is usually looking
  // past what the bounded row already shows.
  const [scope, setScope] = useState<TerminalSwitcherScope>('all')
  useEffect(() => {
    if (isOpen) setScope('all')
  }, [isOpen])

  const context: TerminalSwitcherContext = { terminals, activeProjectId, activeGroup }
  const scopes = availableScopes(context)
  const effectiveScope = scopes.includes(scope) ? scope : 'all'
  const entries = orderByRecency(scopeTerminals(effectiveScope, context), recentTerminalIds)

  // A quick terminal has no project; saying "no project" would read as a
  // project terminal that lost its project.
  const ownerLabel = (terminal: Terminal): string =>
    isQuickTerminal(terminal)
      ? t('switcher.quickTerminal')
      : (projects.find((project) => project.id === terminal.projectId)?.name ??
        t('board.noProject'))

  const jump = (terminal: Terminal): void => {
    onClose()
    openBoardTerminal({ projectId: terminal.projectId ?? null, terminalId: terminal.id, navigate })
    // Same reason as the switcher bar: a quick terminal is shown by its page,
    // and selecting it would repoint the project workspace's active terminal.
    if (!isQuickTerminal(terminal)) useTerminalStore.getState().selectTerminal(terminal.id)
  }

  return (
    <CommandDialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <CommandInput placeholder={t('switcher.searchPlaceholder')} />
      <fieldset
        className="flex items-center gap-0.5 border-0 border-b border-border px-2 py-1.5"
        aria-label={t('switcher.scopeLabel')}
      >
        {scopes.map((candidate) => (
          <button
            key={candidate}
            type="button"
            onClick={() => setScope(candidate)}
            aria-pressed={candidate === effectiveScope}
            className={cn(
              'rounded px-1.5 py-0.5 text-2xs transition-colors duration-150',
              candidate === effectiveScope
                ? 'bg-secondary text-foreground'
                : 'text-muted-foreground hover:text-foreground'
            )}
          >
            {t(`switcher.scope.${candidate}`)}
          </button>
        ))}
      </fieldset>
      <CommandList>
        <CommandEmpty>{t('switcher.empty')}</CommandEmpty>
        <CommandGroup heading={t('switcher.listLabel')}>
          {entries.map((terminal) => (
            <CommandItem
              key={terminal.id}
              // cmdk matches on `value`; the project name is included so
              // "which project was that in" is a usable query too.
              value={`${terminal.name} ${ownerLabel(terminal)}`}
              onSelect={() => jump(terminal)}
            >
              <span
                className={cn(
                  'size-1.5 shrink-0 rounded-full',
                  STATUS_DOT[terminalBoardStatus(terminal)]
                )}
                aria-hidden="true"
              />
              <span className="truncate">{terminal.name}</span>
              <span className="ml-auto truncate pl-2 text-2xs text-muted-foreground">
                {ownerLabel(terminal)}
                {terminal.id === activeTerminalId ? ` · ${t('switcher.current')}` : ''}
              </span>
            </CommandItem>
          ))}
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  )
}
