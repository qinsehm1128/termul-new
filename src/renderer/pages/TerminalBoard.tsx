import { FolderKanban, SquareTerminal } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { useShallow } from 'zustand/shallow'
import {
  ListEmptyState,
  ListPanelHeader,
  ListRow,
  ListRowMeta,
  ListRowStatus,
  type ListRowStatusTone,
  pathBasename
} from '@/components/lists'
import { getColorClasses } from '@/lib/colors'
import {
  buildTerminalBoard,
  countBoardTerminals,
  filterTerminalBoard,
  terminalBoardStatus
} from '@/lib/terminal-board'
import { openBoardProject, openBoardTerminal } from '@/lib/terminal-board-navigation'
import { cn } from '@/lib/utils'
import { useProjectStore } from '@/stores/project-store'
import { useAllTerminals } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'

function statusTone(key: ReturnType<typeof terminalBoardStatus>): ListRowStatusTone {
  if (key === 'attention' || key === 'disconnected') return 'need'
  if (key === 'live') return 'working'
  return 'idle'
}

export default function TerminalBoard(): React.JSX.Element {
  const { t } = useTranslation('terminal')
  const navigate = useNavigate()
  const terminals = useAllTerminals()
  const { projects, groups } = useProjectStore(
    useShallow((state) => ({ projects: state.projects, groups: state.groups }))
  )
  const [query, setQuery] = useState('')

  const board = useMemo(
    () => buildTerminalBoard(terminals, projects, groups),
    [groups, projects, terminals]
  )
  const visible = useMemo(() => filterTerminalBoard(board, query), [board, query])
  const total = countBoardTerminals(board)
  const shown = countBoardTerminals(visible)

  const openTerminal = (terminal: Terminal): void => {
    openBoardTerminal({
      projectId: terminal.projectId ?? null,
      terminalId: terminal.id,
      navigate
    })
  }

  return (
    <section
      className="flex h-full min-h-0 flex-col bg-background"
      aria-labelledby="terminal-board-title"
      data-testid="terminal-board"
    >
      <div className="mx-auto flex w-full max-w-3xl min-h-0 flex-1 flex-col px-4">
        <header className="shrink-0 border-b border-border/70 py-5">
          <h1
            id="terminal-board-title"
            className="text-lg font-semibold tracking-tight text-foreground"
          >
            {t('board.title')}
          </h1>
          <p className="mt-1 max-w-prose text-sm leading-relaxed text-muted-foreground">
            {t('board.description')}
          </p>
        </header>

        <ListPanelHeader
          title={t('board.listTitle')}
          shown={shown}
          total={total}
          search={query}
          onSearchChange={setQuery}
          searchLabel={t('board.search')}
          searchPlaceholder={t('board.searchPlaceholder')}
          clearSearchLabel={t('board.clearSearch')}
          className="border-border/70"
        />

        <div className="min-h-0 flex-1 overflow-y-auto py-3">
          {total === 0 ? (
            <ListEmptyState title={t('board.emptyTitle')} message={t('board.empty')} />
          ) : shown === 0 ? (
            <ListEmptyState title={t('board.noMatchesTitle')} message={t('board.noMatches')} />
          ) : (
            <div className="flex flex-col gap-5">
              {visible.map((group) => {
                const groupLabel =
                  group.groupId === '__unassigned__'
                    ? t('board.unassigned')
                    : group.groupId === null
                      ? t('board.ungrouped')
                      : group.groupName
                const groupColor = group.color ? getColorClasses(group.color) : null
                return (
                  <section
                    key={group.groupId ?? 'ungrouped'}
                    aria-label={groupLabel}
                    data-testid={
                      group.groupId
                        ? `terminal-board-group-${group.groupId}`
                        : 'terminal-board-ungrouped'
                    }
                  >
                    <h2
                      className={cn(
                        'mb-2 px-1.5 text-xs font-semibold tracking-tight',
                        groupColor?.text ?? 'text-foreground'
                      )}
                    >
                      {groupLabel}
                    </h2>
                    <div className="flex flex-col gap-3">
                      {group.projects.map((project) => {
                        const projectColor = getColorClasses(project.color)
                        const canOpenProject = Boolean(project.projectId)
                        return (
                          <div
                            key={project.projectId || 'unassigned'}
                            className="rounded-md border border-border/60 bg-card/30"
                          >
                            <div className="flex items-center gap-2 px-2.5 pt-2">
                              <FolderKanban
                                className={cn('size-3.5 shrink-0', projectColor.text)}
                                aria-hidden="true"
                              />
                              {canOpenProject ? (
                                <button
                                  type="button"
                                  className="min-w-0 truncate text-left text-xs font-medium text-foreground hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                  onClick={() =>
                                    openBoardProject({ projectId: project.projectId, navigate })
                                  }
                                >
                                  {project.projectName}
                                </button>
                              ) : (
                                <span className="min-w-0 truncate text-xs font-medium text-foreground">
                                  {t('board.noProject')}
                                </span>
                              )}
                              {project.archived ? (
                                <span className="text-2xs text-muted-foreground">
                                  {t('board.archived')}
                                </span>
                              ) : null}
                            </div>
                            <div className="py-1">
                              {project.terminals.map((terminal) => {
                                const status = terminalBoardStatus(terminal)
                                return (
                                  <ListRow
                                    key={terminal.id}
                                    density="comfortable"
                                    title={
                                      <span className="inline-flex min-w-0 items-center gap-1.5">
                                        <SquareTerminal
                                          className="size-3.5 shrink-0 text-muted-foreground"
                                          aria-hidden="true"
                                        />
                                        <span className="truncate">{terminal.name}</span>
                                        <ListRowStatus
                                          status={statusTone(status)}
                                          label={t(`board.status.${status}`)}
                                        />
                                      </span>
                                    }
                                    titleAttr={`${groupLabel} / ${project.projectName || t('board.noProject')} / ${terminal.name}`}
                                    preview={
                                      terminal.cwd
                                        ? pathBasename(terminal.cwd) || terminal.cwd
                                        : undefined
                                    }
                                    meta={
                                      <ListRowMeta
                                        items={[
                                          terminal.kind === 'agent'
                                            ? (terminal.agentName ?? terminal.kind)
                                            : terminal.shell,
                                          terminal.cwd
                                        ]}
                                      />
                                    }
                                    onClick={() => openTerminal(terminal)}
                                  />
                                )
                              })}
                            </div>
                          </div>
                        )
                      })}
                    </div>
                  </section>
                )
              })}
            </div>
          )}
        </div>
      </div>
    </section>
  )
}
