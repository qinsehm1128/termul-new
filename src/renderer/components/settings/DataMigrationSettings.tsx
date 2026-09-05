import { AlertTriangle, CheckCircle2, Loader2, PlayCircle } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  type BrandMigrationRootStatus,
  type BrandMigrationRun,
  brandMigrationApi,
  hasFailedRoots,
  type LegacyDataDetection,
  type LegacySignalKind
} from '@/lib/brand-migration-api'
import { logFrontendError } from '@/lib/log-api'
import { cn } from '@/lib/utils'

/**
 * The permanent home of the pre-rename data merge.
 *
 * `BrandMigrationBanner` is the *prompt* — it appears once, at a moment the
 * user did not choose, and goes away. This panel is the place the same
 * operation can always be found: to run it for the first time after dismissing
 * the banner, to retry a root that failed, or just to read what the last pass
 * actually did. The merge is copy-only and skips destinations that already
 * exist, so re-running it is free and the button is never disabled on the
 * grounds of "already done".
 *
 * Unlike the banner this lists every root, including the absent ones. The
 * banner filters to what is actionable because it is interrupting; a panel the
 * user opened on purpose is where "we looked at this and there was nothing"
 * is the useful answer.
 */

/** Display order — the copying roots first, then the read-in-place ones. */
const DISPLAY_ORDER = [
  'appDataDir',
  'keychainService',
  'standaloneStateRoot',
  'localStorage',
  'documentsWorkspace',
  'repoWorkspaceDir',
  'sshKnownHosts'
] as const satisfies readonly LegacySignalKind[]

/**
 * Compile-time proof the order covers every kind: a kind added to
 * `LegacySignalKind` but not listed here would silently vanish from the panel.
 */
type UnplacedKinds = Exclude<LegacySignalKind, (typeof DISPLAY_ORDER)[number]>
const _everyKindIsDisplayed: [UnplacedKinds] extends [never] ? true : UnplacedKinds = true
void _everyKindIsDisplayed

type RowStatus = BrandMigrationRootStatus | 'pending'

const STATUS_STYLES: Record<RowStatus, string> = {
  pending: 'bg-amber-500/10 text-amber-500 ring-amber-500/20',
  migrated: 'bg-green-500/10 text-green-500 ring-green-500/20',
  skipped: 'bg-secondary/50 text-muted-foreground ring-border/70',
  notApplicable: 'bg-secondary/50 text-muted-foreground ring-border/70',
  failed: 'bg-red-500/10 text-red-500 ring-red-500/20'
}

interface PanelState {
  detection: LegacyDataDetection | null
  lastRun: BrandMigrationRun | null
}

/**
 * What one root's row says right now.
 *
 * The journal wins wherever it has an entry — it is the record of what the app
 * actually did, and it outranks a probe that only reports what is still lying
 * around. Falling back to the probe covers the host that has never merged.
 */
function rowStatus(kind: LegacySignalKind, state: PanelState): RowStatus {
  const recorded = state.lastRun?.roots.find((root) => root.kind === kind)
  if (recorded) return recorded.status
  if (kind === 'sshKnownHosts') return state.detection?.sshKnownHosts.state ?? 'notApplicable'
  const signal = state.detection?.signals.find((entry) => entry.kind === kind)
  return signal?.present ? 'pending' : 'notApplicable'
}

function rowReason(kind: LegacySignalKind, state: PanelState): string | null {
  const recorded = state.lastRun?.roots.find((root) => root.kind === kind)
  if (recorded?.reason) return recorded.reason
  if (kind === 'sshKnownHosts' && state.detection?.sshKnownHosts.state === 'failed') {
    return state.detection.sshKnownHosts.reason
  }
  return null
}

export function DataMigrationSettings(): React.JSX.Element {
  const { t } = useTranslation('settings')
  const [state, setState] = useState<PanelState>({ detection: null, lastRun: null })
  const [running, setRunning] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const refresh = useCallback(async (): Promise<void> => {
    const [detection, lastRun] = await Promise.all([
      brandMigrationApi.detectLegacyData(),
      brandMigrationApi.lastRun()
    ])
    if (mounted.current) setState({ detection, lastRun })
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const start = useCallback(async (): Promise<void> => {
    setRunning(true)
    setError(null)
    try {
      await brandMigrationApi.runMigration()
      // Re-read rather than trusting the receipt in hand: the journal is what
      // every other surface reads, so a disagreement between them would be
      // invisible here and visible everywhere else.
      await refresh()
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      if (mounted.current) setError(message)
      void logFrontendError({ source: 'DataMigrationSettings', message })
    } finally {
      if (mounted.current) setRunning(false)
    }
  }, [refresh])

  const { detection, lastRun } = state
  const signals = detection?.signals ?? []
  const failed = hasFailedRoots(lastRun)

  return (
    <div className="w-2/3 space-y-4" data-testid="data-migration-settings">
      <div className="rounded-md bg-secondary/25 px-3 py-2.5 shadow-[inset_0_1px_0_0_hsl(var(--foreground)/0.035)]">
        <p className="text-xs text-muted-foreground" data-testid="data-migration-summary">
          {lastRun === null
            ? t('dataMigration.neverRun')
            : t('dataMigration.lastRunAt', {
                when: new Date(lastRun.startedAtUtc).toLocaleString()
              })}
        </p>
        <p className="mt-1 text-xs font-medium text-foreground">{t('dataMigration.copyOnly')}</p>
      </div>

      {failed && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-md border border-red-500/20 bg-red-500/10 px-3 py-2.5"
        >
          <AlertTriangle size={14} className="mt-0.5 flex-shrink-0 text-red-500" />
          <p className="text-xs text-foreground">{t('dataMigration.lastRunFailed')}</p>
        </div>
      )}

      {error && (
        <div
          role="alert"
          data-testid="data-migration-error"
          className="flex items-start gap-2 rounded-md border border-red-500/20 bg-red-500/10 px-3 py-2.5"
        >
          <AlertTriangle size={14} className="mt-0.5 flex-shrink-0 text-red-500" />
          <p className="text-xs text-foreground">{error}</p>
        </div>
      )}

      <div className="space-y-2">
        {DISPLAY_ORDER.map((kind) => {
          const status = rowStatus(kind, state)
          const reason = rowReason(kind, state)
          const signal = signals.find((entry) => entry.kind === kind)
          return (
            <div
              key={kind}
              data-migration-row={kind}
              data-migration-status={status}
              className="rounded-md bg-secondary/25 px-3 py-2.5 shadow-[inset_0_1px_0_0_hsl(var(--foreground)/0.035)]"
            >
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0 flex-1">
                  <span className="text-sm text-foreground">
                    {t(`dataMigration.roots.${kind}`, signal?.label ?? kind)}
                  </span>
                  <p className="mt-0.5 break-all font-mono text-3xs text-muted-foreground/70">
                    {signal?.path ?? t('dataMigration.pathUnavailable')}
                  </p>
                  {reason && <p className="mt-1 text-xs text-muted-foreground">{reason}</p>}
                </div>
                <span
                  className={cn(
                    'flex-shrink-0 rounded-full px-1.5 py-0.5 text-3xs ring-1 ring-inset',
                    STATUS_STYLES[status]
                  )}
                >
                  {t(`dataMigration.status.${status}`)}
                </span>
              </div>
            </div>
          )
        })}
      </div>

      {/* The plan roots with no row of their own. They are decisions, not work,
          and the journal is the only place they are recorded. */}
      {lastRun !== null && lastRun.notices.length > 0 && (
        <div className="space-y-1" data-testid="data-migration-notices">
          {lastRun.notices.map((notice) => (
            <p key={notice.id} className="text-xs text-muted-foreground">
              <span className="font-mono text-3xs">{notice.id}</span> {notice.detail}
            </p>
          ))}
        </div>
      )}

      {detection?.tccNotice != null && (
        <div className="rounded-md bg-secondary/25 px-3 py-2.5">
          <p className="text-xs font-medium text-foreground">{t('dataMigration.tccTitle')}</p>
          <p className="mt-0.5 text-xs text-muted-foreground">{detection.tccNotice}</p>
        </div>
      )}

      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => void start()}
          disabled={running}
          data-testid="data-migration-start"
          className="inline-flex h-8 items-center gap-2 rounded-md bg-secondary/50 px-3 text-sm text-foreground transition-colors duration-150 hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-50"
        >
          {running ? (
            <Loader2 size={14} className="animate-spin" />
          ) : lastRun === null ? (
            <PlayCircle size={14} />
          ) : (
            <CheckCircle2 size={14} />
          )}
          {running
            ? t('dataMigration.running')
            : lastRun === null
              ? t('dataMigration.start')
              : t('dataMigration.rerun')}
        </button>
      </div>

      <p className="text-xs text-muted-foreground">{t('dataMigration.repeatable')}</p>
    </div>
  )
}
