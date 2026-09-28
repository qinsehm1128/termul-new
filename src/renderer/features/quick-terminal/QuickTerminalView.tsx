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

/** The host resizes the PTY to the real viewport once the terminal mounts. */
const INITIAL_COLS = 80
const INITIAL_ROWS = 24

type Phase =
  | { kind: 'opening' }
  | { kind: 'ready'; ptyId: string }
  | { kind: 'exited'; ptyId: string }
  | { kind: 'failed'; message: string }

/** The live shell of one quick terminal. Remount (via `key`) per quick terminal. */
export function QuickTerminalView({ record }: { record: QuickTerminalRecord }): React.JSX.Element {
  const { t } = useTranslation('quickTerminal')
  const open = useQuickTerminalStore((state) => state.open)
  const [phase, setPhase] = useState<Phase>({ kind: 'opening' })
  const current = useRef(0)

  const attach = useCallback(async () => {
    const attempt = ++current.current
    setPhase({ kind: 'opening' })
    const result = await open(record.id, INITIAL_COLS, INITIAL_ROWS)
    if (attempt !== current.current) return
    if (result.success && result.data) {
      setPhase({ kind: 'ready', ptyId: result.data.terminalId })
    } else {
      setPhase({ kind: 'failed', message: failureMessage(result, t('openFailed')) })
    }
  }, [open, record.id, t])

  useEffect(() => {
    void attach()
    return () => {
      current.current += 1
    }
  }, [attach])

  if (phase.kind === 'opening') {
    return (
      <div
        role="status"
        className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground"
      >
        <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
        {t('opening')}
      </div>
    )
  }
  if (phase.kind === 'failed') {
    return (
      <div role="alert" className="flex h-full flex-col items-center justify-center gap-3 text-sm">
        <p className="text-destructive">{phase.message}</p>
        <Button size="sm" variant="outline" onClick={() => void attach()}>
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
          <Button size="sm" variant="outline" onClick={() => void attach()}>
            <RotateCcw className="mr-1.5 size-3.5" aria-hidden="true" />
            {t('reopen')}
          </Button>
        </div>
      ) : null}
    </div>
  )
}
