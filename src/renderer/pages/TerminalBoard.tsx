import type { TFunction } from 'i18next'
import { FolderKanban } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
import { useShallow } from 'zustand/shallow'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import { ListEmptyState, ListPanelHeader, ListScopeSwitch } from '@/components/lists'
import { BoardTerminalPreview } from '@/components/terminal-board/BoardTerminalPreview'
import { BoardTerminalRow, boardRowDomId } from '@/components/terminal-board/BoardTerminalRow'
import { useConversationStore } from '@/features/agent-session/stores/conversation-store'
import { getColorClasses } from '@/lib/colors'
import { buildConversationTerminalNames } from '@/lib/conversation-terminal-names'
import {
  BOARD_STATUS_BUCKETS,
  type BoardRowFacts,
  type BoardStatusBucket,
  boardStatusBucket,
  buildTerminalBoard,
  compareBoardRows,
  countBoardTerminals,
  filterTerminalBoard,
  latestActivity,
  type TerminalBoardGroupBlock,
  type TerminalBoardProjectBlock,
  terminalRowState
} from '@/lib/terminal-board'
import { openBoardProject, openBoardTerminal } from '@/lib/terminal-board-navigation'
import { lastTerminalOutputAt } from '@/lib/terminal-output-clock'
import { cn } from '@/lib/utils'
import { useProjectStore } from '@/stores/project-store'
import { useAllTerminals, useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import { isQuickTerminal, isSshTerminal, type Terminal } from '@/types/project'

/** How often working/idle and "last active" are re-evaluated. */
export const BOARD_TICK_MS = 2000
/** Below this width there is no room for the preview, so a click opens. */
const PREVIEW_MIN_WIDTH_PX = 1024

type StatusFilter = 'all' | BoardStatusBucket
type GroupBy = 'project' | 'status'
type PendingAction = { kind: 'terminate' | 'restart'; terminal: Terminal }

interface BoardRowEntry {
  terminal: Terminal
  facts: BoardRowFacts
  projectLabel: string
}

interface ProjectSection {
  key: string
  project: TerminalBoardProjectBlock
  /** Hidden when the group already says it ("Quick terminals" twice). */
  showHeader: boolean
  rows: BoardRowEntry[]
}

interface Section {
  key: string
  label: string
  labelClass?: string
  testId?: string
  projects: ProjectSection[]
}

function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs)
    return () => clearInterval(timer)
  }, [intervalMs])
  return now
}

function useMinWidth(px: number): boolean {
  const query = `(min-width: ${px}px)`
  const supported = typeof window !== 'undefined' && typeof window.matchMedia === 'function'
  const [matches, setMatches] = useState(() => supported && window.matchMedia(query).matches)
  useEffect(() => {
    if (!supported) return
    const list = window.matchMedia(query)
    const update = (): void => setMatches(list.matches)
    update()
    list.addEventListener('change', update)
    return () => list.removeEventListener('change', update)
  }, [query, supported])
  return matches
}

/** Project and Conversation terminals can be restarted or ended here; quick and SSH ones on their own pages. */
function isManageable(terminal: Terminal): boolean {
  return Boolean(terminal.ptyId) && !isQuickTerminal(terminal) && !isSshTerminal(terminal)
}

export default function TerminalBoard(): React.JSX.Element {
  const { t } = useTranslation('terminal')
  const navigate = useNavigate()
  const terminals = useAllTerminals()
  const { projects, groups } = useProjectStore(
    useShallow((state) => ({ projects: state.projects, groups: state.groups }))
  )
  const [query, setQuery] = useState('')
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all')
  const [groupBy, setGroupBy] = useState<GroupBy>('project')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null)
  const [actionLoading, setActionLoading] = useState(false)
  const now = useNow(BOARD_TICK_MS)
  const wide = useMinWidth(PREVIEW_MIN_WIDTH_PX)
  const rootRef = useRef<HTMLElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  const conversationSummaries = useConversationStore((state) => state.summariesById)
  const conversationGroupName = t('switcher.conversations')
  const quickGroupName = t('board.quickTerminals')
  const sshGroupName = t('board.sshTerminals')

  const conversationNames = useMemo(
    () =>
      buildConversationTerminalNames(
        terminals,
        conversationSummaries,
        t('switcher.untitledConversation', { defaultValue: 'Untitled conversation' })
      ),
    [terminals, conversationSummaries, t]
  )

  const board = useMemo(
    () =>
      buildTerminalBoard(terminals, projects, groups, conversationNames, conversationGroupName, {
        quick: quickGroupName,
        ssh: sshGroupName
      }),
    [
      groups,
      projects,
      terminals,
      conversationNames,
      conversationGroupName,
      quickGroupName,
      sshGroupName
    ]
  )
  const searched = useMemo(() => filterTerminalBoard(board, query), [board, query])
  const total = countBoardTerminals(board)

  const groupLabel = useCallback(
    (group: TerminalBoardGroupBlock): string =>
      group.groupId === '__unassigned__'
        ? t('board.unassigned')
        : group.groupId === null
          ? t('board.ungrouped')
          : group.groupName,
    [t]
  )

  // Facts for every searched row; the status counts are taken before the
  // status filter so each chip says what picking it would show.
  const rowsByGroup = useMemo(
    () =>
      searched.map((group) => ({
        group,
        projects: group.projects.map((project) => ({
          project,
          rows: project.terminals.map((terminal): BoardRowEntry => {
            const lastOutputAt = lastTerminalOutputAt(terminal.ptyId)
            return {
              terminal,
              facts: {
                state: terminalRowState(terminal, lastOutputAt, now),
                lastActiveAt: latestActivity(terminal, lastOutputAt)
              },
              projectLabel: project.projectName || groupLabel(group)
            }
          })
        }))
      })),
    [searched, now, groupLabel]
  )

  const counts = useMemo(() => {
    const result: Record<BoardStatusBucket, number> = {
      attention: 0,
      working: 0,
      idle: 0,
      ended: 0
    }
    for (const { projects: blocks } of rowsByGroup) {
      for (const { rows } of blocks) {
        for (const row of rows) result[boardStatusBucket(row.facts.state)] += 1
      }
    }
    return result
  }, [rowsByGroup])

  const sections = useMemo((): Section[] => {
    const keep = (row: BoardRowEntry): boolean =>
      statusFilter === 'all' || boardStatusBucket(row.facts.state) === statusFilter
    const byFacts = (a: BoardRowEntry, b: BoardRowEntry): number =>
      compareBoardRows(a.facts, b.facts)

    if (groupBy === 'status') {
      return BOARD_STATUS_BUCKETS.filter(
        (bucket) => statusFilter === 'all' || bucket === statusFilter
      )
        .map((bucket) => {
          const rows = rowsByGroup
            .flatMap(({ projects: blocks }) => blocks.flatMap(({ rows: list }) => list))
            .filter((row) => boardStatusBucket(row.facts.state) === bucket)
            .sort(byFacts)
          return {
            key: `status-${bucket}`,
            label: t(`board.filter.${bucket}`),
            testId: `terminal-board-status-${bucket}`,
            projects:
              rows.length > 0
                ? [
                    {
                      key: bucket,
                      project: {
                        projectId: '',
                        projectName: '',
                        color: 'gray' as const,
                        archived: false,
                        terminals: rows.map((row) => row.terminal)
                      },
                      showHeader: false,
                      rows
                    }
                  ]
                : []
          }
        })
        .filter((section) => section.projects.length > 0)
    }

    return rowsByGroup
      .map(({ group, projects: blocks }) => {
        const label = groupLabel(group)
        const groupColor = group.color ? getColorClasses(group.color) : null
        return {
          key: group.groupId ?? 'ungrouped',
          label,
          labelClass: groupColor?.text,
          testId: group.groupId
            ? `terminal-board-group-${group.groupId}`
            : 'terminal-board-ungrouped',
          projects: blocks
            .map(({ project, rows }) => ({
              key: project.conversationId ?? (project.projectId || 'unassigned'),
              project,
              showHeader: Boolean(project.projectName) && project.projectName !== label,
              rows: rows.filter(keep).sort(byFacts)
            }))
            .filter((block) => block.rows.length > 0)
        }
      })
      .filter((section) => section.projects.length > 0)
  }, [groupBy, groupLabel, rowsByGroup, statusFilter, t])

  // Display order, which is also keyboard order.
  const ordered = useMemo(
    () => sections.flatMap((section) => section.projects.flatMap((block) => block.rows)),
    [sections]
  )
  const shown = ordered.length
  const selected = ordered.find((row) => row.terminal.id === selectedId) ?? null

  // With room for a preview, something is always previewed: a selection that
  // left the list (closed, filtered out) falls back to the first row.
  useEffect(() => {
    if (!wide) return
    if (selected === null && ordered.length > 0) setSelectedId(ordered[0].terminal.id)
  }, [wide, selected, ordered])

  useEffect(() => {
    if (!selectedId) return
    document.getElementById(boardRowDomId(selectedId))?.scrollIntoView?.({ block: 'nearest' })
  }, [selectedId])

  const openTerminal = useCallback(
    (terminal: Terminal): void => {
      openBoardTerminal({
        projectId: terminal.projectId ?? null,
        terminalId: terminal.id,
        navigate
      })
    },
    [navigate]
  )

  const selectRow = (terminal: Terminal): void => {
    if (wide) setSelectedId(terminal.id)
    else openTerminal(terminal)
  }

  const moveSelection = (delta: number): void => {
    if (ordered.length === 0) return
    const index = ordered.findIndex((row) => row.terminal.id === selectedId)
    const next =
      index === -1
        ? delta > 0
          ? 0
          : ordered.length - 1
        : Math.min(ordered.length - 1, Math.max(0, index + delta))
    setSelectedId(ordered[next].terminal.id)
  }

  const handleKeyDown = (event: React.KeyboardEvent<HTMLElement>): void => {
    const inSearch = event.target instanceof HTMLInputElement
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      moveSelection(event.key === 'ArrowDown' ? 1 : -1)
      if (inSearch) listRef.current?.focus()
      return
    }
    if (event.key === 'Enter' && selected && !(event.target instanceof HTMLButtonElement)) {
      event.preventDefault()
      openTerminal(selected.terminal)
      return
    }
    if (event.key === '/' && !inSearch) {
      event.preventDefault()
      rootRef.current?.querySelector<HTMLInputElement>('input[type="search"]')?.focus()
    }
  }

  const confirmPendingAction = async (): Promise<void> => {
    if (!pendingAction) return
    const { kind, terminal } = pendingAction
    setActionLoading(true)
    try {
      const store = useTerminalStore.getState()
      if (kind === 'terminate') {
        const ok = await store.terminateTerminalResource(terminal.id)
        if (ok) useWorkspaceStore.getState().closeTerminalView(terminal.id)
        else toast.error(t('lifecycle.terminateFailed'))
      } else if (!(await store.restartTerminalResource(terminal.id))) {
        toast.error(t('lifecycle.restartFailed'))
      }
    } finally {
      setActionLoading(false)
      setPendingAction(null)
    }
  }

  const filterOptions: Array<{ value: StatusFilter; label: string }> = [
    { value: 'all', label: `${t('board.filter.all')} ${total}` },
    ...BOARD_STATUS_BUCKETS.map((bucket) => ({
      value: bucket as StatusFilter,
      label: `${t(`board.filter.${bucket}`)} ${counts[bucket]}`
    }))
  ]

  return (
    <section
      ref={rootRef}
      className="flex h-full min-h-0 flex-col bg-background"
      aria-labelledby="terminal-board-title"
      data-testid="terminal-board"
      onKeyDown={handleKeyDown}
    >
      <div className="mx-auto flex w-full max-w-6xl min-h-0 flex-1 flex-col px-4">
        <header className="shrink-0 py-4">
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
          actions={
            <ListScopeSwitch<GroupBy>
              value={groupBy}
              onChange={setGroupBy}
              ariaLabel={t('board.groupBy.label')}
              options={[
                { value: 'project', label: t('board.groupBy.project') },
                { value: 'status', label: t('board.groupBy.status') }
              ]}
            />
          }
        >
          <ListScopeSwitch<StatusFilter>
            value={statusFilter}
            onChange={setStatusFilter}
            ariaLabel={t('board.filter.label')}
            options={filterOptions}
          />
        </ListPanelHeader>

        <div className="flex min-h-0 flex-1 gap-4 py-3">
          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            {total === 0 ? (
              <ListEmptyState title={t('board.emptyTitle')} message={t('board.empty')} />
            ) : shown === 0 ? (
              <ListEmptyState title={t('board.noMatchesTitle')} message={t('board.noMatches')} />
            ) : (
              <div
                ref={listRef}
                role="listbox"
                tabIndex={0}
                aria-label={t('board.listTitle')}
                aria-activedescendant={selected ? boardRowDomId(selected.terminal.id) : undefined}
                className="min-h-0 flex-1 overflow-y-auto rounded-md focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring/50"
              >
                {sections.map((section) => (
                  <section
                    key={section.key}
                    aria-label={section.label}
                    data-testid={section.testId}
                    className="mb-4"
                  >
                    <h2
                      className={cn(
                        'px-2 pb-1 text-2xs font-semibold uppercase tracking-wide',
                        section.labelClass ?? 'text-muted-foreground'
                      )}
                    >
                      {section.label}
                    </h2>
                    {section.projects.map((block) => (
                      <div key={block.key} className="mb-1.5">
                        {block.showHeader ? (
                          <ProjectHeader
                            block={block.project}
                            count={block.rows.length}
                            t={t}
                            onOpen={(projectId) => openBoardProject({ projectId, navigate })}
                          />
                        ) : null}
                        {block.rows.map((row) => (
                          <BoardTerminalRow
                            key={row.terminal.id}
                            terminal={row.terminal}
                            facts={row.facts}
                            now={now}
                            selected={row.terminal.id === selected?.terminal.id}
                            projectLabel={groupBy === 'status' ? row.projectLabel : undefined}
                            t={t}
                            onSelect={() => selectRow(row.terminal)}
                            onOpen={() => openTerminal(row.terminal)}
                            onRestart={
                              isManageable(row.terminal)
                                ? () =>
                                    setPendingAction({ kind: 'restart', terminal: row.terminal })
                                : undefined
                            }
                            onTerminate={
                              isManageable(row.terminal) && row.facts.state !== 'exited'
                                ? () =>
                                    setPendingAction({ kind: 'terminate', terminal: row.terminal })
                                : undefined
                            }
                          />
                        ))}
                      </div>
                    ))}
                  </section>
                ))}
              </div>
            )}
            <p className="shrink-0 pt-2 text-2xs text-muted-foreground">
              {t('board.keyboardHint')}
            </p>
          </div>
          {wide && selected ? (
            <div className="flex min-h-0 w-[26rem] shrink-0 flex-col">
              <BoardTerminalPreview
                terminal={selected.terminal}
                facts={selected.facts}
                projectLabel={selected.projectLabel}
                t={t}
                onOpen={() => openTerminal(selected.terminal)}
              />
            </div>
          ) : null}
        </div>
      </div>

      <ConfirmDialog
        isOpen={pendingAction !== null}
        title={
          pendingAction?.kind === 'restart'
            ? t('board.restartTitle', { name: pendingAction.terminal.name })
            : t('lifecycle.terminateTitle')
        }
        message={
          pendingAction?.kind === 'restart'
            ? t('board.restartDescription')
            : t('lifecycle.terminateDescription')
        }
        confirmLabel={
          pendingAction?.kind === 'restart'
            ? t('board.actions.restart')
            : t('lifecycle.terminateProcess')
        }
        cancelLabel={t('board.cancel')}
        variant="danger"
        isLoading={actionLoading}
        onConfirm={() => void confirmPendingAction()}
        onCancel={() => {
          if (!actionLoading) setPendingAction(null)
        }}
      />
    </section>
  )
}

function ProjectHeader({
  block,
  count,
  t,
  onOpen
}: {
  block: TerminalBoardProjectBlock
  count: number
  t: TFunction<'terminal'>
  onOpen: (projectId: string) => void
}): React.JSX.Element {
  const color = getColorClasses(block.color)
  return (
    <div className="flex h-7 items-center gap-1.5 px-2">
      <FolderKanban className={cn('size-3.5 shrink-0', color.text)} aria-hidden="true" />
      {block.projectId ? (
        <button
          type="button"
          tabIndex={-1}
          className="min-w-0 truncate text-left text-xs font-medium text-foreground hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          onClick={() => onOpen(block.projectId)}
        >
          {block.projectName}
        </button>
      ) : (
        <span className="min-w-0 truncate text-xs font-medium text-foreground">
          {block.projectName || t('board.noProject')}
        </span>
      )}
      <span className="text-2xs tabular-nums text-muted-foreground">{count}</span>
      {block.archived ? (
        <span className="text-2xs text-muted-foreground">{t('board.archived')}</span>
      ) : null}
    </div>
  )
}
