import type { TFunction } from 'i18next'
import { ArrowUpRight } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { readTerminalPreview } from '@/components/terminal/terminal-preview'
import type { BoardRowFacts } from '@/lib/terminal-board'
import type { Terminal } from '@/types/project'

/** How often the preview re-reads the screen while it is open. */
export const BOARD_PREVIEW_REFRESH_MS = 1000

type PreviewState = { status: 'loading' } | { status: 'ready'; lines: string[] | null }

export interface BoardTerminalPreviewProps {
  terminal: Terminal
  facts: BoardRowFacts
  projectLabel: string
  t: TFunction<'terminal'>
  onOpen: () => void
}

/**
 * The selected terminal's current screen, so a glance answers "what is it
 * asking?" without leaving the board. Reads only what the renderer already
 * holds, once a second and only for this one terminal.
 */
export function BoardTerminalPreview({
  terminal,
  facts,
  projectLabel,
  t,
  onOpen
}: BoardTerminalPreviewProps): React.JSX.Element {
  const [preview, setPreview] = useState<PreviewState>({ status: 'loading' })
  // The record is rebuilt on every store update; polling keys off identity.
  const terminalRef = useRef(terminal)
  terminalRef.current = terminal

  // biome-ignore lint/correctness/useExhaustiveDependencies: restart polling only for another terminal or PTY; the record itself is read through the ref.
  useEffect(() => {
    let cancelled = false
    setPreview({ status: 'loading' })
    const refresh = (): void => {
      void readTerminalPreview(terminalRef.current)
        .catch(() => null)
        .then((lines) => {
          if (!cancelled) setPreview({ status: 'ready', lines })
        })
    }
    refresh()
    const timer = setInterval(refresh, BOARD_PREVIEW_REFRESH_MS)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [terminal.id, terminal.ptyId])

  const lines = preview.status === 'ready' ? preview.lines : null
  return (
    <aside
      aria-label={t('board.preview.label', { name: terminal.name })}
      className="flex min-h-0 flex-col rounded-md border border-border/60 bg-card/30"
    >
      <header className="flex items-center gap-2 border-b border-border/60 px-3 py-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13px] font-medium text-foreground">{terminal.name}</div>
          <div className="truncate text-2xs text-muted-foreground">
            {t(`board.state.${facts.state}`)}
            {projectLabel ? ` · ${projectLabel}` : ''}
          </div>
        </div>
        <button
          type="button"
          onClick={onOpen}
          className="inline-flex h-7 shrink-0 items-center gap-1 rounded-md bg-secondary px-2 text-xs text-secondary-foreground hover:bg-secondary/80 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <ArrowUpRight className="size-3.5" aria-hidden="true" />
          {t('board.actions.open')}
        </button>
      </header>
      {/* Bottom-aligned: the newest rows are the ones worth seeing. */}
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden px-3 py-2">
        {preview.status === 'loading' ? null : lines && lines.length > 0 ? (
          <pre
            data-testid="terminal-board-preview-screen"
            className="mt-auto whitespace-pre font-mono text-2xs leading-4 text-foreground/90"
          >
            {lines.join('\n')}
          </pre>
        ) : (
          <p className="text-xs text-muted-foreground">{t('board.preview.empty')}</p>
        )}
      </div>
    </aside>
  )
}
