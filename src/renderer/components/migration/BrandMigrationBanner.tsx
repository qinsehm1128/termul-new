import { brandCanonical } from '@shared/brand'
import { AlertTriangle, CheckCircle2, LoaderCircle, ShieldAlert } from 'lucide-react'
import { useCallback, useEffect, useId, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import {
  type BrandMigrationReceipt,
  type BrandMigrationRootStatus,
  brandMigrationApi,
  type LegacyDataDetection,
  type LegacyDataSignal
} from '@/lib/brand-migration-api'
import { logFrontendError } from '@/lib/log-api'
import { cn } from '@/lib/utils'

/**
 * One-time, user-initiated merge entry point for data left behind by the
 * previous brand.
 *
 * The product decision this encodes: the app *detects* legacy data, *asks*, and
 * merges only when the user says so. There is no silent sweep. The copy states
 * that merging never deletes the old data, because that is the property that
 * makes saying yes safe.
 *
 * Two things are deliberately NOT symmetric here:
 *
 * - `sshKnownHosts` was already migrated during startup (host-key checking is
 *   fail-closed without it, so it cannot wait for a prompt). It is listed with
 *   its startup outcome, never as pending work, and "Start merge" does not
 *   re-run it.
 * - When that startup migration FAILED, the warning is a separate region with
 *   its own name and its own consequence sentence, and "Later" does not hide
 *   it. "Later" defers a merge the user can still do; it must not defer a
 *   security posture that has already changed underneath them.
 */

/**
 * Session-scoped dismissal. `sessionStorage` is the right lifetime: "Later"
 * means "not now", not "never" — the prompt is due again on the next app start,
 * and a reload inside the same run should not re-nag.
 */
const DISMISS_STORAGE_KEY = `${brandCanonical().storageKeyPrefix}brand-migration-dismissed`

function canUseSessionStorage(): boolean {
  try {
    return typeof sessionStorage !== 'undefined'
  } catch {
    // Accessing sessionStorage can throw in sandboxed / privacy-restricted contexts.
    return false
  }
}

function readDismissed(): boolean {
  if (!canUseSessionStorage()) return false
  try {
    return sessionStorage.getItem(DISMISS_STORAGE_KEY) === '1'
  } catch {
    return false
  }
}

function persistDismissed(): void {
  if (!canUseSessionStorage()) return
  try {
    sessionStorage.setItem(DISMISS_STORAGE_KEY, '1')
  } catch {
    // Best-effort: an in-memory dismissal still holds for this mount.
  }
}

/** Status → translation key. Spelled out so the key set stays statically typed. */
const STATUS_LABEL_KEYS = {
  pending: 'brandMigration.status.pending',
  migrated: 'brandMigration.status.migrated',
  skipped: 'brandMigration.status.skipped',
  notApplicable: 'brandMigration.status.notApplicable',
  failed: 'brandMigration.status.failed'
} as const

/**
 * The status shown next to one root.
 *
 * Once a run has produced a receipt, the receipt is authoritative for EVERY
 * row — `sshKnownHosts` included. That row is not a second migration: the host
 * carries it verbatim from the startup pass. Rendering it rather than masking
 * it with the locally-known startup value is deliberate, and it is what makes a
 * host-side re-run *visible* instead of silent — an overwritten status shows up
 * on screen next to a warning that still reports the startup outcome.
 *
 * Before any run there is no receipt, and `sshKnownHosts` reports the startup
 * outcome directly, so it never reads "pending" in the pre-merge list. Every
 * other root is "pending" until a receipt says otherwise.
 */
function rootStatus(
  signal: LegacyDataSignal,
  detection: LegacyDataDetection,
  receipt: BrandMigrationReceipt | null
): BrandMigrationRootStatus | 'pending' {
  const entry = receipt?.roots.find((root) => root.kind === signal.kind)
  if (entry) return entry.status
  if (signal.kind === 'sshKnownHosts') return detection.sshKnownHosts.state
  return 'pending'
}

export function BrandMigrationBanner(): React.JSX.Element | null {
  const { t } = useTranslation('common')
  const headingId = useId()
  const sshHeadingId = useId()
  const [detection, setDetection] = useState<LegacyDataDetection | null>(null)
  const [dismissed, setDismissed] = useState<boolean>(() => readDismissed())
  const [receipt, setReceipt] = useState<BrandMigrationReceipt | null>(null)
  const [running, setRunning] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void brandMigrationApi.detectLegacyData().then((result) => {
      if (cancelled) return
      setDetection(result)
    })
    return () => {
      cancelled = true
    }
  }, [])

  const startMigration = useCallback(async (): Promise<void> => {
    setRunning(true)
    setError(null)
    try {
      setReceipt(await brandMigrationApi.runMigration())
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      void logFrontendError({
        level: 'error',
        source: 'brand-migration.run',
        message
      })
    } finally {
      setRunning(false)
    }
  }, [])

  const dismiss = useCallback((): void => {
    persistDismissed()
    setDismissed(true)
  }, [])

  const presentSignals = useMemo(
    () => detection?.signals.filter((signal) => signal.present) ?? [],
    [detection]
  )

  if (!detection) return null

  const sshFailure = detection.sshKnownHosts.state === 'failed' ? detection.sshKnownHosts : null
  // The merge half of the banner. "Later" closes it; so does a run the user has
  // already acknowledged by dismissing the receipt.
  const showMergePrompt = detection.hasLegacyData && !dismissed

  // The SSH failure outlives the merge prompt, so an otherwise-empty banner
  // still renders when that startup migration failed.
  if (!showMergePrompt && !sshFailure) return null

  return (
    <section
      aria-labelledby={showMergePrompt ? headingId : undefined}
      data-testid="brand-migration-banner"
      className="fixed left-1/2 top-2 z-[75] w-[min(34rem,calc(100vw-1.5rem))] -translate-x-1/2 space-y-3 rounded-md border border-border bg-background/95 p-3 text-xs shadow-md backdrop-blur sm:top-3 sm:text-sm"
    >
      {showMergePrompt && (
        <div className="space-y-2">
          <h2 id={headingId} className="flex items-center gap-2 font-medium">
            <AlertTriangle aria-hidden="true" className="size-4 shrink-0 text-amber-500" />
            {t('brandMigration.title')}
          </h2>
          <p className="text-muted-foreground">{t('brandMigration.description')}</p>
          <p className="font-medium" data-testid="brand-migration-copy-only">
            {t('brandMigration.copyOnly')}
          </p>

          <p className="text-muted-foreground">{t('brandMigration.rootsLabel')}</p>
          <ul data-testid="brand-migration-roots" className="space-y-1">
            {presentSignals.map((signal) => {
              const status = rootStatus(signal, detection, receipt)
              return (
                <li
                  key={signal.kind}
                  data-testid="brand-migration-root"
                  data-kind={signal.kind}
                  data-status={status}
                  className="flex items-baseline justify-between gap-2"
                >
                  <span className="min-w-0">
                    <span className="font-medium">{signal.label}</span>{' '}
                    <span className="break-all font-mono text-[10px] text-muted-foreground sm:text-xs">
                      {signal.path ?? t('brandMigration.pathUnavailable')}
                    </span>
                  </span>
                  <span
                    className={cn(
                      'shrink-0 text-muted-foreground',
                      status === 'failed' && 'text-destructive'
                    )}
                  >
                    {t(STATUS_LABEL_KEYS[status])}
                  </span>
                </li>
              )
            })}
          </ul>

          {detection.tccNotice !== null && (
            <div data-testid="brand-migration-tcc" className="rounded border border-border/70 p-2">
              <p className="font-medium">{t('brandMigration.tccTitle')}</p>
              <p className="text-muted-foreground">{detection.tccNotice}</p>
            </div>
          )}

          {receipt !== null && (
            <p
              data-testid="brand-migration-done"
              className="flex items-center gap-2 font-medium text-emerald-600"
            >
              <CheckCircle2 aria-hidden="true" className="size-4 shrink-0" />
              {t('brandMigration.doneTitle')}
            </p>
          )}

          {error !== null && (
            <p role="alert" data-testid="brand-migration-error" className="text-destructive">
              {t('brandMigration.failedTitle')} — {t('brandMigration.reason', { reason: error })}
            </p>
          )}

          <div className="flex items-center justify-end gap-2 pt-1">
            <Button type="button" variant="ghost" size="sm" onClick={dismiss}>
              {t('brandMigration.later')}
            </Button>
            <Button
              type="button"
              size="sm"
              disabled={running || receipt !== null}
              onClick={() => {
                void startMigration()
              }}
            >
              {running && <LoaderCircle aria-hidden="true" className="animate-spin" />}
              {running ? t('brandMigration.running') : t('brandMigration.start')}
            </Button>
          </div>
        </div>
      )}

      {sshFailure && (
        <div
          role="alert"
          aria-labelledby={sshHeadingId}
          data-testid="brand-migration-ssh-warning"
          className="space-y-1 rounded border border-destructive bg-destructive/5 p-2 text-destructive"
        >
          <h3 id={sshHeadingId} className="flex items-center gap-2 font-semibold">
            <ShieldAlert aria-hidden="true" className="size-4 shrink-0" />
            {t('brandMigration.ssh.failedTitle')}
          </h3>
          <p>{t('brandMigration.ssh.failedBody')}</p>
          <p className="break-all font-mono text-[10px] sm:text-xs">
            {t('brandMigration.reason', { reason: sshFailure.reason })}
          </p>
        </div>
      )}
    </section>
  )
}
