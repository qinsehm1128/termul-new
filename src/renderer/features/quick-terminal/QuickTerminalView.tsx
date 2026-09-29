import type { QuickTerminalRecord } from '@shared/types/quick-terminal.types'
import { LoaderCircle, RotateCcw } from 'lucide-react'
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { failureMessage, useQuickTerminalStore } from './quick-terminal-store'

const ConnectedTerminal = lazy(() =>
  import('@/components/terminal/ConnectedTerminal').then((module) => ({
    default: module.ConnectedTerminal
  }))
)

type Phase =
  // A throwaway terminal is mounted to learn the viewport's grid, so the shell
  // starts at the size it is shown at. Starting it smaller and resizing it
  // right after makes zsh redraw its prompt against text xterm has already
  // reflowed, which leaves a duplicate prompt behind.
  | { kind: 'measuring' }
  | { kind: 'opening' }
  | { kind: 'ready'; ptyId: string }
  | { kind: 'exited'; ptyId: string }
  | { kind: 'failed'; message: string }

/** The live shell of one quick terminal. Remount (via `key`) per quick terminal. */
export function QuickTerminalView({ record }: { record: QuickTerminalRecord }): React.JSX.Element {
  const { t } = useTranslation('quickTerminal')
  const open = useQuickTerminalStore((state) => state.open)
  const [phase, setPhase] = useState<Phase>({ kind: 'measuring' })
  const current = useRef(0)

  const attach = useCallback(
    async (cols: number, rows: number) => {
      const attempt = ++current.current
      setPhase({ kind: 'opening' })
      const result = await open(record.id, cols, rows)
      if (attempt !== current.current) return
      if (result.success && result.data) {
        setPhase({ kind: 'ready', ptyId: result.data.terminalId })
      } else {
        setPhase({ kind: 'failed', message: failureMessage(result, t('openFailed')) })
      }
    },
    [open, record.id, t]
  )

  useEffect(
    () => () => {
      current.current += 1
    },
    []
  )

  const reopen = (): void => setPhase({ kind: 'measuring' })

  if (phase.kind === 'measuring' || phase.kind === 'opening') {
    return (
      <div className="relative h-full min-h-0">
        {phase.kind === 'measuring' ? (
          <Suspense fallback={null}>
            <ConnectedTerminal
              autoSpawn={false}
              isVisible
              autoFocus={false}
              onInitialGrid={(cols, rows) => void attach(cols, rows)}
            />
          </Suspense>
        ) : null}
        <div
          role="status"
          className="absolute inset-0 flex items-center justify-center gap-2 bg-background text-sm text-muted-foreground"
        >
          <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
          {t('opening')}
        </div>
      </div>
    )
  }
  if (phase.kind === 'failed') {
    return (
      <div role="alert" className="flex h-full flex-col items-center justify-center gap-3 text-sm">
        <p className="text-destructive">{phase.message}</p>
        <Button size="sm" variant="outline" onClick={reopen}>
          <RotateCcw className="mr-1.5 size-3.5" aria-hidden="true" />
          {t('reopen')}
        </Button>
      </div>
    )
  }
  return (
    <div className="relative h-full min-h-0">
      <Suspense fallback={null}>
        <ConnectedTerminal
          key={phase.ptyId}
          terminalId={phase.ptyId}
          storeTerminalId={phase.ptyId}
          autoSpawn={false}
          isVisible
          autoFocus
          onExit={() => setPhase({ kind: 'exited', ptyId: phase.ptyId })}
        />
      </Suspense>
      {phase.kind === 'exited' ? (
        <div className="absolute inset-x-0 bottom-0 flex items-center justify-between gap-3 border-t bg-background/95 px-4 py-2 text-sm">
          <span className="text-muted-foreground">{t('exited')}</span>
          <Button size="sm" variant="outline" onClick={reopen}>
            <RotateCcw className="mr-1.5 size-3.5" aria-hidden="true" />
            {t('reopen')}
          </Button>
        </div>
      ) : null}
    </div>
  )
}
