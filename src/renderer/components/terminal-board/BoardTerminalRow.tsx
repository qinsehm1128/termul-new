import { AlertTriangle, ArrowUpRight, CircleStop, RotateCw, Square, Unplug } from 'lucide-react'
import type { TFunction } from 'i18next'
import { formatRelative } from '@/i18n/format'
import {
  type BoardRowFacts,
  type TerminalRowState,
  terminalActivityText
} from '@/lib/terminal-board'
import { cn } from '@/lib/utils'
import type { Terminal } from '@/types/project'

export function boardRowDomId(terminalId: string): string {
  return `terminal-board-row-${terminalId}`
}

function StateIcon({ state }: { state: TerminalRowState }): React.JSX.Element {
  switch (state) {
    case 'attention':
      return <AlertTriangle className="size-3.5 text-amber-500" aria-hidden="true" />
    case 'disconnected':
      return <Unplug className="size-3.5 text-destructive" aria-hidden="true" />
    case 'working':
      return (
        <span className="flex size-3.5 items-center justify-center" aria-hidden="true">
          <span className="size-2 animate-pulse rounded-full bg-emerald-500" />
        </span>
      )
    case 'idle':
      return (
        <span className="flex size-3.5 items-center justify-center" aria-hidden="true">
          <span className="size-2 rounded-full border border-muted-foreground/70" />
        </span>
      )
    case 'exited':
      return <CircleStop className="size-3.5 text-muted-foreground/70" aria-hidden="true" />
  }
}

/** "just now" under ten seconds, then the coarsest unit that fits. */
export function formatSince(
  lastActiveAt: number | undefined,
  now: number,
  justNow: string
): string {
  if (lastActiveAt === undefined) return ''
  const seconds = Math.max(0, Math.round((now - lastActiveAt) / 1000))
  if (seconds < 10) return justNow
  const options = { numeric: 'auto', style: 'narrow' } as const
  if (seconds < 60) return formatRelative(-seconds, 'second', options)
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return formatRelative(-minutes, 'minute', options)
  const hours = Math.round(minutes / 60)
  if (hours < 24) return formatRelative(-hours, 'hour', options)
  return formatRelative(-Math.round(hours / 24), 'day', options)
}

function RowAction({
  label,
  onClick,
  children
}: {
  label: string
  onClick: () => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <button
      type="button"
      tabIndex={-1}
      aria-label={label}
      title={label}
      onClick={(event) => {
        // The row itself selects on click; an action must not also do that.
        event.stopPropagation()
        onClick()
      }}
      className="inline-flex size-6 items-center justify-center rounded text-muted-foreground hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    >
      {children}
    </button>
  )
}

export interface BoardTerminalRowProps {
  terminal: Terminal
  facts: BoardRowFacts
  now: number
  selected: boolean
  /** Shown when rows are not already listed under their project. */
  projectLabel?: string
  t: TFunction<'terminal'>
  onSelect: () => void
  onOpen: () => void
  onRestart?: () => void
  onTerminate?: () => void
}

/**
 * One terminal on one line: state, name, what it is doing, when it last did
 * something. Shell, full path and the like stay in the tooltip — repeated on
 * every row they pushed each terminal to three lines.
 */
export function BoardTerminalRow({
  terminal,
  facts,
  now,
  selected,
  projectLabel,
  t,
  onSelect,
  onOpen,
  onRestart,
  onTerminate
}: BoardTerminalRowProps): React.JSX.Element {
  const stateLabel = t(`board.state.${facts.state}`)
  const activity = terminalActivityText(terminal)
  const tooltip = [terminal.name, stateLabel, terminal.shell, terminal.cwd]
    .filter(Boolean)
    .join(' · ')
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents lint/a11y/useFocusableInteractive: the listbox owns focus and keys, and points at this row through aria-activedescendant.
    <div
      id={boardRowDomId(terminal.id)}
      role="option"
      aria-selected={selected}
      title={tooltip}
      data-state={facts.state}
      onClick={onSelect}
      onDoubleClick={onOpen}
      className={cn(
        'group/board-row flex h-9 cursor-default items-center gap-2 rounded-md px-2 text-[13px] transition-colors duration-100',
        selected ? 'bg-sidebar-accent text-foreground' : 'hover:bg-sidebar-accent/50'
      )}
    >
      <StateIcon state={facts.state} />
      <span className="sr-only">{stateLabel}</span>
      <span className="min-w-0 max-w-[16rem] shrink-0 truncate font-medium text-foreground">
        {terminal.name}
      </span>
      {terminal.kind === 'agent' ? (
        <span className="shrink-0 rounded bg-secondary/60 px-1.5 py-px text-2xs text-secondary-foreground">
          {terminal.agentName ?? t('board.agent')}
        </span>
      ) : null}
      <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{activity}</span>
      {projectLabel ? (
        <span className="max-w-[10rem] shrink-0 truncate text-2xs text-muted-foreground">
          {projectLabel}
        </span>
      ) : null}
      <span className="w-20 shrink-0 text-right text-2xs tabular-nums text-muted-foreground">
        {formatSince(facts.lastActiveAt, now, t('board.justNow'))}
      </span>
      <span
        className={cn(
          'flex shrink-0 items-center gap-0.5 transition-opacity duration-100',
          selected ? 'opacity-100' : 'opacity-0 group-hover/board-row:opacity-100'
        )}
      >
        <RowAction label={t('board.actions.open')} onClick={onOpen}>
          <ArrowUpRight className="size-3.5" aria-hidden="true" />
        </RowAction>
        {onRestart ? (
          <RowAction label={t('board.actions.restart')} onClick={onRestart}>
            <RotateCw className="size-3.5" aria-hidden="true" />
          </RowAction>
        ) : null}
        {onTerminate ? (
          <RowAction label={t('board.actions.terminate')} onClick={onTerminate}>
            <Square className="size-3.5" aria-hidden="true" />
          </RowAction>
        ) : null}
      </span>
    </div>
  )
}
