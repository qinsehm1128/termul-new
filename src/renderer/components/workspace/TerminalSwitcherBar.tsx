import { PanelRight } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import type { TerminalBoardStatusKey } from '@/lib/terminal-board'
import { openBoardTerminal } from '@/lib/terminal-board-navigation'
import {
  groupTerminalsByProject,
  scopeTerminals,
  TERMINAL_BAR_SCOPES,
  type TerminalBarScope,
  type TerminalSwitcherContext
} from '@/lib/terminal-switcher'
import { cn } from '@/lib/utils'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'

const STATUS_DOT: Record<TerminalBoardStatusKey, string> = {
  live: 'bg-emerald-500',
  hidden: 'bg-muted-foreground/60',
  disconnected: 'bg-destructive',
  attention: 'bg-amber-500'
}

/**
 * A one-line project switcher above the pane area.
 *
 * It lives here and not in either rail because the left rail already owns the
 * project list and the right rail already stacks the CLI panel, the file tree
 * and the SSH explorer — the strip above the panes is the only place that
 * competes with neither.
 *
 * One chip per project, not per terminal. The tab bar directly below already
 * lists the current project's terminals, so a terminal-granular strip here
 * duplicated it and, when terminals are named after their project, rendered
 * visibly identical neighbouring chips. Terminal granularity lives in the tab
 * bar (within a project) and the quick switcher (across projects).
 */
interface TerminalSwitcherBarProps {
  isListOpen?: boolean
  onToggleList?: () => void
}

export function TerminalSwitcherBar({
  isListOpen = false,
  onToggleList
}: TerminalSwitcherBarProps = {}): React.JSX.Element | null {
  const { t } = useTranslation('terminal')
  const navigate = useNavigate()

  const terminals = useTerminalStore((state) => state.terminals)
  const activeTerminalId = useTerminalStore((state) => state.activeTerminalId)
  const recentTerminalIds = useTerminalStore((state) => state.recentTerminalIds)
  const activeProjectId = useProjectStore((state) => state.activeProjectId)
  const activeGroupId = useProjectStore((state) => state.activeGroupId)
  const groups = useProjectStore((state) => state.groups)
  const projects = useProjectStore((state) => state.projects)

  // Membership, not navigation mode. `activeGroupId` tracks the sidebar's
  // group *scope*, which `selectProject` deliberately exits — so clicking any
  // project, even a sibling in the same group, switches it off. Keying the step
  // off that alone left it permanently unlit for anyone who works by clicking
  // projects. The group a user is "in" is the one owning the current project.
  const activeGroup = useMemo(
    () =>
      groups.find((group) => group.id === activeGroupId) ??
      groups.find((group) => group.projectIds.includes(activeProjectId)) ??
      null,
    [groups, activeGroupId, activeProjectId]
  )

  const [scope, setScope] = useState<TerminalBarScope>('all')

  // Clicking a group in the sidebar is itself the request to see that group's
  // terminals, so the row follows the selection rather than making the user
  // re-pick the scope. Only a *change* flips it, so a manual scope choice
  // survives unrelated re-renders.
  const lastGroupIdRef = useRef(activeGroupId)
  useEffect(() => {
    if (activeGroupId && activeGroupId !== lastGroupIdRef.current) setScope('group')
    lastGroupIdRef.current = activeGroupId
  }, [activeGroupId])

  const context: TerminalSwitcherContext = { terminals, activeProjectId, activeGroup }
  // `group` stops being selectable when no group is selected; fall back rather
  // than blank the row.
  const effectiveScope: TerminalBarScope = scope === 'group' && !activeGroup ? 'all' : scope
  const visible = groupTerminalsByProject(
    scopeTerminals(effectiveScope, context),
    projects,
    recentTerminalIds,
    t('switcher.unassigned')
  )

  // Both steps are always on screen, with `group` disabled until a group is
  // selected. Hiding it instead made the control look absent rather than
  // inapplicable — the same reason the whole bar is no longer hidden below two
  // terminals: one terminal still needs a visible way to reach the list.
  if (terminals.length === 0) return null

  const activeProjectOfTerminal = terminals.find(
    (terminal) => terminal.id === activeTerminalId
  )?.projectId

  const openTerminal = (terminalId: string, projectId: string | undefined): void => {
    openBoardTerminal({ projectId: projectId ?? null, terminalId, navigate })
    useTerminalStore.getState().selectTerminal(terminalId)
  }

  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border/70 bg-sidebar px-2">
      <fieldset
        className="flex shrink-0 items-center gap-0.5 rounded-md border-0 bg-muted/50 p-0.5"
        aria-label={t('switcher.scopeLabel')}
      >
        {TERMINAL_BAR_SCOPES.map((candidate) => {
          const selectable = candidate !== 'group' || Boolean(activeGroup)
          return (
            <button
              key={candidate}
              type="button"
              onClick={() => setScope(candidate)}
              disabled={!selectable}
              title={selectable ? undefined : t('switcher.scopeUnavailable')}
              aria-pressed={candidate === effectiveScope}
              className={cn(
                'rounded px-1.5 py-0.5 text-2xs transition-colors duration-150',
                !selectable
                  ? 'cursor-not-allowed text-muted-foreground/40'
                  : candidate === effectiveScope
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'
              )}
            >
              {t(`switcher.scope.${candidate}`)}
            </button>
          )
        })}
      </fieldset>

      {onToggleList ? (
        <button
          type="button"
          onClick={onToggleList}
          aria-pressed={isListOpen}
          title={t(isListOpen ? 'switcher.hideList' : 'switcher.showList')}
          aria-label={t(isListOpen ? 'switcher.hideList' : 'switcher.showList')}
          className={cn(
            'order-last shrink-0 rounded p-1 transition-colors duration-150',
            isListOpen
              ? 'bg-secondary text-foreground'
              : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'
          )}
        >
          <PanelRight className="size-3.5" aria-hidden="true" />
        </button>
      ) : null}

      {visible.length === 0 ? (
        <span className="truncate text-2xs text-muted-foreground">{t('switcher.empty')}</span>
      ) : (
        <div
          className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto"
          role="tablist"
          aria-label={t('switcher.listLabel')}
        >
          {visible.map((entry) => {
            const isActive = entry.projectId === activeProjectOfTerminal
            return (
              <button
                key={entry.projectId ?? '__unassigned__'}
                type="button"
                role="tab"
                aria-selected={isActive}
                onClick={() => openTerminal(entry.targetTerminalId, entry.projectId)}
                // `terminals`, not `count`: i18next reads `count` as a plural
                // selector and would look for keys we do not define.
                title={t('switcher.projectTitle', {
                  project: entry.name,
                  terminals: entry.terminals.length
                })}
                className={cn(
                  'inline-flex h-6 shrink-0 items-center gap-1.5 rounded-md px-2 text-2xs transition-colors duration-150 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
                  isActive
                    ? 'bg-secondary text-foreground'
                    : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'
                )}
              >
                <span
                  className={cn('size-1.5 shrink-0 rounded-full', STATUS_DOT[entry.status])}
                  aria-hidden="true"
                />
                <span className="max-w-32 truncate">{entry.name}</span>
                <span className="shrink-0 tabular-nums text-muted-foreground/70" aria-hidden="true">
                  {entry.terminals.length}
                </span>
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
