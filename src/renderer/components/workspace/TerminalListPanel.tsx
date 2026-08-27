import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { AgentIcon } from '@/components/agents/AgentIcon'
import type { TerminalBoardStatusKey } from '@/lib/terminal-board'
import { buildTerminalBoard, terminalBoardStatus } from '@/lib/terminal-board'
import { openBoardTerminal } from '@/lib/terminal-board-navigation'
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
 * Vertical terminal list docked at the right edge of the pane area.
 *
 * Deliberately not in the right rail: that rail already stacks the CLI panel,
 * the file tree and the SSH explorer, and a fourth tenant would just fight
 * them for width. Living inside the centre column keeps it independent.
 *
 * Unlike the switcher row this is never scoped — the row is bounded on
 * purpose, so a list that repeated the same subset would earn none of the
 * space it costs. Grouping is the board's, so both surfaces agree on where a
 * terminal belongs.
 */
export function TerminalListPanel(): React.JSX.Element {
  const { t } = useTranslation('terminal')
  const navigate = useNavigate()

  const terminals = useTerminalStore((state) => state.terminals)
  const activeTerminalId = useTerminalStore((state) => state.activeTerminalId)
  const projects = useProjectStore((state) => state.projects)
  const groups = useProjectStore((state) => state.groups)

  const board = useMemo(
    () => buildTerminalBoard(terminals, projects, groups),
    [terminals, projects, groups]
  )

  const open = (terminalId: string, projectId: string | undefined): void => {
    openBoardTerminal({ projectId: projectId ?? null, terminalId, navigate })
    useTerminalStore.getState().selectTerminal(terminalId)
  }

  return (
    <aside
      className="flex w-44 shrink-0 flex-col overflow-y-auto border-l border-border/70 bg-sidebar py-1"
      aria-label={t('switcher.listPanelLabel')}
    >
      {terminals.length === 0 ? (
        <p className="px-2 py-1 text-2xs text-muted-foreground">{t('board.empty')}</p>
      ) : (
        board.map((group) =>
          group.projects.map((project) => (
            <div key={`${group.groupId ?? 'none'}:${project.projectId}`} className="mb-1">
              <p className="truncate px-2 py-0.5 text-2xs text-muted-foreground">
                {project.projectName}
              </p>
              {project.terminals.map((terminal) => (
                <button
                  key={terminal.id}
                  type="button"
                  onClick={() => open(terminal.id, terminal.projectId)}
                  title={terminal.name}
                  aria-current={terminal.id === activeTerminalId ? 'true' : undefined}
                  className={cn(
                    'flex h-6 w-full items-center gap-1.5 px-2 text-left text-2xs transition-colors duration-150',
                    terminal.id === activeTerminalId
                      ? 'bg-sidebar-accent text-foreground'
                      : 'text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground'
                  )}
                >
                  <span
                    className={cn(
                      'size-1.5 shrink-0 rounded-full',
                      STATUS_DOT[terminalBoardStatus(terminal)]
                    )}
                    aria-hidden="true"
                  />
                  {terminal.agentId ? (
                    <AgentIcon
                      agentId={terminal.agentId}
                      name={terminal.agentName}
                      className="size-3"
                    />
                  ) : null}
                  <span className="truncate">{terminal.name}</span>
                </button>
              ))}
            </div>
          ))
        )
      )}
    </aside>
  )
}
