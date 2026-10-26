import type { IpcResult } from '@shared/types/ipc.types'
import type { QuickTerminalOpened, QuickTerminalRecord } from '@shared/types/quick-terminal.types'
import { LoaderCircle, Power, RotateCcw } from 'lucide-react'
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { failureMessage, useQuickTerminalStore } from './quick-terminal-store'

const ConnectedTerminal = lazy(() =>
  import('@/components/terminal/ConnectedTerminal').then((module) => ({
    default: module.ConnectedTerminal
  }))
)

// Re-entering an entry while its measured spawn is pending must join it, not
// launch a second shell. Requests outlive React mounts, just like the PTYs.
const pendingAttaches = new Map<string, Promise<IpcResult<QuickTerminalOpened>>>()

type Phase =
  // A throwaway terminal is mounted to learn the viewport's grid, so the shell
  // starts at the size it is shown at. Starting it smaller and resizing it
  // right after makes zsh redraw its prompt against text xterm has already
  // reflowed, which leaves a duplicate prompt behind.
  | { kind: 'measuring' }
  | { kind: 'opening' }
  | { kind: 'ready'; ptyId: string }
  | { kind: 'exited'; ptyId: string }
  // The user closed the shell; the quick terminal stays until reopened.
  | { kind: 'closed' }
  | { kind: 'failed'; message: string; dead?: boolean }

/** The live shell of one quick terminal. Remount (via `key`) per quick terminal. */
export function QuickTerminalView({
  record,
  terminalId,
  createNew = false,
  isVisible = true,
  onOpened,
  tabId = 'default'
}: {
  record: QuickTerminalRecord
  terminalId?: string
  createNew?: boolean
  isVisible?: boolean
  onOpened?: (terminalId: string) => void
  tabId?: string
}): React.JSX.Element {
  const { t } = useTranslation('quickTerminal')
  const open = useQuickTerminalStore((state) => state.open)
  const createTerminal = useQuickTerminalStore((state) => state.createTerminal)
  const close = useQuickTerminalStore((state) => state.closeTerminal)
  const [phase, setPhase] = useState<Phase>({ kind: 'measuring' })
  const [closing, setClosing] = useState(false)
  const current = useRef(0)
  const inFlight = useRef(false)
  const openedPty = useRef<string | null>(null)
  const createOnReopen = useRef(false)
  const openedCallback = useRef(onOpened)
  openedCallback.current = onOpened

  const attach = useCallback(
    async (cols: number, rows: number) => {
      if (inFlight.current || openedPty.current) return
      inFlight.current = true
      const attempt = ++current.current
      setPhase({ kind: 'opening' })
      const requestKey = `${record.id}:${tabId}`
      let pending = pendingAttaches.get(requestKey)
      if (!pending) {
        pending =
          (createNew && !terminalId) || createOnReopen.current
            ? createTerminal(record.id, cols, rows)
            : open(record.id, cols, rows, terminalId)
        pendingAttaches.set(requestKey, pending)
      }
      const result = await pending
      if (pendingAttaches.get(requestKey) === pending) pendingAttaches.delete(requestKey)
      inFlight.current = false
      // Bind even if this entry was navigated away from while the host spawned.
      if (result.success) openedCallback.current?.(result.data.terminalId)
      if (attempt !== current.current) return
      if (result.success && result.data) {
        openedPty.current = result.data.terminalId
        setPhase({ kind: 'ready', ptyId: result.data.terminalId })
      } else {
        setPhase({
          kind: 'failed',
          message: failureMessage(result, t('openFailed')),
          dead: !result.success && result.code === 'QUICK_TERMINAL_INVALID_TARGET'
        })
      }
    },
    [open, createTerminal, createNew, terminalId, record.id, tabId, t]
  )

  useEffect(
    () => () => {
      current.current += 1
    },
    []
  )

  const reopen = (): void => {
    // Once this tab is dead, every explicit retry must create its replacement.
    // Falling back to open() could attach a sibling (or retry the pruned ID).
    createOnReopen.current ||= phase.kind === 'exited' || (phase.kind === 'failed' && !!phase.dead)
    openedPty.current = null
    setPhase({ kind: 'measuring' })
  }

  const closeShell = async (): Promise<void> => {
    setClosing(true)
    if (phase.kind !== 'ready') return
    const result = await close(record.id, phase.ptyId)
    setClosing(false)
    if (!result.success) toast.error(failureMessage(result, t('closeFailed')))
  }

  const reopenButton = (
    <Button size="sm" variant="outline" onClick={reopen}>
      <RotateCcw className="mr-1.5 size-3.5" aria-hidden="true" />
      {t('reopen')}
    </Button>
  )

  let body: React.JSX.Element
  if (phase.kind === 'measuring' || phase.kind === 'opening') {
    body = (
      <>
        {phase.kind === 'measuring' ? (
          <Suspense fallback={null}>
            <ConnectedTerminal
              autoSpawn={false}
              isVisible={isVisible}
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
      </>
    )
  } else if (phase.kind === 'failed' || phase.kind === 'closed') {
    body = (
      <div
        role={phase.kind === 'failed' ? 'alert' : 'status'}
        className="flex h-full flex-col items-center justify-center gap-3 text-sm"
      >
        <p className={phase.kind === 'failed' ? 'text-destructive' : 'text-muted-foreground'}>
          {phase.kind === 'failed' ? phase.message : t('closed')}
        </p>
        {reopenButton}
      </div>
    )
  } else {
    body = (
      <>
        <Suspense fallback={null}>
          <ConnectedTerminal
            key={phase.ptyId}
            terminalId={phase.ptyId}
            storeTerminalId={phase.ptyId}
            autoSpawn={false}
            isVisible={isVisible}
            autoFocus={isVisible}
            onExit={() => setPhase({ kind: 'exited', ptyId: phase.ptyId })}
          />
        </Suspense>
        {phase.kind === 'exited' ? (
          <div className="absolute inset-x-0 bottom-0 flex items-center justify-between gap-3 border-t bg-background/95 px-4 py-2 text-sm">
            <span className="text-muted-foreground">{t('exited')}</span>
            {reopenButton}
          </div>
        ) : null}
      </>
    )
  }

  // One frame for every phase: the grid measured before the shell starts has
  // to match the one it is shown in, so the toolbar never comes and goes.
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-9 shrink-0 items-center justify-between gap-3 border-b px-3">
        <span className="truncate text-xs text-muted-foreground" title={record.cwd}>
          {record.cwd}
        </span>
        <Button
          size="sm"
          variant="ghost"
          className="h-7 shrink-0"
          disabled={phase.kind !== 'ready' || closing}
          onClick={() => void closeShell()}
        >
          <Power className="mr-1.5 size-3.5" aria-hidden="true" />
          {t('close')}
        </Button>
      </div>
      <div className="relative min-h-0 flex-1">{body}</div>
    </div>
  )
}
