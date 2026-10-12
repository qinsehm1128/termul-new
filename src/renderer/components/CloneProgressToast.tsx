import { AlertTriangle, ChevronDown, ChevronRight, GitBranch, Loader2, X } from 'lucide-react'
import { useEffect, useState, useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { i18n } from '@/i18n'
import { GIT_CLONE_CANCELLED, type GitCloneProgress } from '@/lib/git-api'

/**
 * A clone can take minutes. A spinner alone could not tell a slow transfer
 * from a stuck one, so this card shows the phase, percentage, transfer speed
 * and elapsed time, keeps git's output a click away, and can cancel.
 */

/** Without new output for this long the card says the connection may be stuck. */
export const CLONE_STALL_WARNING_MS = 30_000

const MAX_LOG_LINES = 200

export interface CloneViewState {
  startedAt: number
  lastProgressAt: number
  phase: string | null
  percent: number | null
  transferred: string | null
  speed: string | null
  log: string[]
  cancelling: boolean
}

/** A redrawn progress line replaces the previous state of its phase. */
export function applyCloneProgress(
  state: CloneViewState,
  progress: GitCloneProgress,
  now: number
): CloneViewState {
  const log = [...state.log]
  const previous = log.at(-1)
  const samePhase =
    progress.phase !== null &&
    previous !== undefined &&
    previous.replace(/^remote: /, '').startsWith(`${progress.phase}:`)
  if (samePhase) log.pop()
  log.push(progress.line)
  const next: CloneViewState = {
    ...state,
    lastProgressAt: now,
    log: log.slice(-MAX_LOG_LINES)
  }
  if (progress.phase !== null) {
    next.phase = progress.phase
    next.percent = progress.percent
    next.transferred = progress.transferred
    next.speed = progress.speed
  }
  return next
}

function createCloneStore(now: number) {
  let state: CloneViewState = {
    startedAt: now,
    lastProgressAt: now,
    phase: null,
    percent: null,
    transferred: null,
    speed: null,
    log: [],
    cancelling: false
  }
  const listeners = new Set<() => void>()
  const set = (next: CloneViewState) => {
    state = next
    for (const listener of listeners) listener()
  }
  return {
    get: () => state,
    subscribe: (listener: () => void) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    progress: (progress: GitCloneProgress) => set(applyCloneProgress(state, progress, Date.now())),
    cancelling: () => set({ ...state, cancelling: true })
  }
}

type CloneStore = ReturnType<typeof createCloneStore>

const PHASE_KEYS: Record<string, 'preparing' | 'receiving' | 'resolving' | 'checkout'> = {
  'Enumerating objects': 'preparing',
  'Counting objects': 'preparing',
  'Compressing objects': 'preparing',
  'Receiving objects': 'receiving',
  'Resolving deltas': 'resolving',
  'Updating files': 'checkout',
  'Checking out files': 'checkout'
}

function phaseLabel(phase: string | null): string {
  if (phase === null) return i18n.t('cloneProgress.phase.connecting', { ns: 'projects' })
  const key = PHASE_KEYS[phase]
  return key ? i18n.t(`cloneProgress.phase.${key}`, { ns: 'projects' }) : phase
}

export function formatElapsed(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000))
  const minutes = Math.floor(seconds / 60)
  return `${String(minutes).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`
}

function CloneProgressCard({
  repoName,
  store,
  onCancel
}: {
  repoName: string
  store: CloneStore
  onCancel: () => void
}): React.JSX.Element {
  const state = useSyncExternalStore(store.subscribe, store.get)
  const [now, setNow] = useState(() => Date.now())
  const [showLog, setShowLog] = useState(false)

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [])

  const silentFor = now - state.lastProgressAt
  const facts = [
    state.transferred,
    state.speed,
    i18n.t('cloneProgress.elapsed', { ns: 'projects', time: formatElapsed(now - state.startedAt) })
  ].filter(Boolean)

  return (
    <div
      data-testid="clone-progress"
      className="w-[356px] rounded-md border border-border bg-popover p-3 text-popover-foreground shadow-lg"
    >
      <div className="flex items-start gap-2">
        {state.cancelling ? (
          <Loader2 size={16} className="mt-0.5 flex-shrink-0 animate-spin text-muted-foreground" />
        ) : (
          <GitBranch size={16} className="mt-0.5 flex-shrink-0 text-primary" />
        )}
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">
            {state.cancelling
              ? i18n.t('cloneProgress.cancelling', { ns: 'projects', name: repoName })
              : i18n.t('cloning', { ns: 'projects', name: repoName })}
          </p>
          <p className="mt-0.5 text-xs text-muted-foreground" data-testid="clone-progress-phase">
            {phaseLabel(state.phase)}
            {state.percent !== null ? ` ${state.percent}%` : ''}
          </p>
        </div>
        <button
          type="button"
          onClick={onCancel}
          disabled={state.cancelling}
          data-testid="clone-progress-cancel"
          className="inline-flex h-6 flex-shrink-0 items-center gap-1 rounded px-1.5 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:opacity-50"
        >
          <X size={12} />
          {i18n.t('cloneProgress.cancel', { ns: 'projects' })}
        </button>
      </div>

      <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-secondary">
        <div
          className="h-full rounded-full bg-primary transition-[width] duration-300"
          style={{ width: `${state.percent ?? 0}%` }}
        />
      </div>
      <p
        className="mt-1.5 font-mono text-3xs text-muted-foreground"
        data-testid="clone-progress-facts"
      >
        {facts.join(' · ')}
      </p>

      {!state.cancelling && silentFor >= CLONE_STALL_WARNING_MS && (
        <div
          role="alert"
          data-testid="clone-progress-stalled"
          className="mt-2 flex items-start gap-1.5 rounded border border-amber-500/20 bg-amber-500/10 px-2 py-1.5"
        >
          <AlertTriangle size={12} className="mt-0.5 flex-shrink-0 text-amber-500" />
          <p className="text-3xs text-foreground">
            {i18n.t('cloneProgress.stalled', {
              ns: 'projects',
              seconds: Math.floor(silentFor / 1000)
            })}
          </p>
        </div>
      )}

      <button
        type="button"
        onClick={() => setShowLog((open) => !open)}
        data-testid="clone-progress-log-toggle"
        className="mt-2 inline-flex items-center gap-1 text-3xs text-muted-foreground hover:text-foreground"
      >
        {showLog ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        {i18n.t('cloneProgress.log', { ns: 'projects' })}
      </button>
      {showLog && (
        <pre
          data-testid="clone-progress-log"
          className="mt-1 max-h-36 overflow-auto whitespace-pre-wrap break-all rounded bg-background/60 p-1.5 font-mono text-3xs text-muted-foreground"
        >
          {state.log.length > 0 ? state.log.join('\n') : '…'}
        </pre>
      )}
    </div>
  )
}

export interface CloneWithProgressOptions {
  repoName: string
  clone: (cloneId: string, onProgress: (progress: GitCloneProgress) => void) => Promise<string>
  cancel: (cloneId: string) => Promise<unknown>
}

/**
 * Run a clone behind a progress card. Resolves to the cloned folder, or null
 * when the clone failed or was cancelled — both are reported here.
 */
export async function runCloneWithProgress({
  repoName,
  clone,
  cancel
}: CloneWithProgressOptions): Promise<string | null> {
  const cloneId = crypto.randomUUID()
  const toastId = `git-clone-${cloneId}`
  const store = createCloneStore(Date.now())
  const requestCancel = () => {
    store.cancelling()
    void cancel(cloneId)
  }
  toast.custom(
    () => <CloneProgressCard repoName={repoName} store={store} onCancel={requestCancel} />,
    {
      id: toastId,
      duration: Infinity
    }
  )

  try {
    const clonedPath = await clone(cloneId, store.progress)
    toast.dismiss(toastId)
    toast.success(i18n.t('cloned', { ns: 'projects', name: repoName }))
    return clonedPath
  } catch (error) {
    toast.dismiss(toastId)
    const message = error instanceof Error ? error.message : String(error)
    if (message === GIT_CLONE_CANCELLED) {
      toast(i18n.t('cloneProgress.cancelled', { ns: 'projects', name: repoName }))
    } else {
      toast.error(i18n.t('cloneFailed', { ns: 'projects', message }))
    }
    return null
  }
}
