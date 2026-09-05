import { brandCanonical } from '@shared/brand'
import { AlertTriangle, CheckCircle2, LoaderCircle, ShieldAlert } from 'lucide-react'
import { useCallback, useEffect, useId, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import {
  type BrandMigrationReceipt,
  type BrandMigrationRootStatus,
  type BrandMigrationRun,
  brandMigrationApi,
  hasFailedRoots,
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
 *
 * # Why the prompt is not keyed on `hasLegacyData` alone
 *
 * It used to be, and that was a bug with no exit: the merge never deletes a
 * legacy root, so `hasLegacyData` is still true the instant after a successful
 * pass and stays true for the rest of the install's life. "Later" is
 * session-scoped on purpose, so nothing suppressed the prompt across restarts
 * either — the banner returned at every single start, forever, to a user with
 * nothing left to do.
 *
 * The journal is the only durable record that the work happened, so the prompt
 * asks it: a recorded run with no failed root means the merge is done and the
 * banner stays away. A run that *did* fail keeps prompting, because that user
 * really does have data that has not come across — and for them "Don't show
 * again" is offered, since a root that cannot be carried may never succeed and
 * Settings → Data migration is a permanent way back in.
 */

/**
 * Session-scoped dismissal. `sessionStorage` is the right lifetime: "Later"
 * means "not now", not "never" — the prompt is due again on the next app start,
 * and a reload inside the same run should not re-nag.
 */
const DISMISS_STORAGE_KEY = `${brandCanonical().storageKeyPrefix}brand-migration-dismissed`

/**
 * Permanent dismissal, for the user whose merge keeps failing on a root that
 * will not budge. `localStorage`, so it outlives the run — the opposite
 * lifetime to the key above, and the reason the two are separate keys.
 */
const SILENCED_STORAGE_KEY = `${brandCanonical().storageKeyPrefix}brand-migration-silenced`

/** `undefined` when the area is unreachable (sandboxed / privacy-restricted). */
function storageArea(kind: 'session' | 'local'): Storage | undefined {
  try {
    const area = kind === 'session' ? sessionStorage : localStorage
    return typeof area === 'undefined' ? undefined : area
  } catch {
    return undefined
  }
}

function readFlag(kind: 'session' | 'local', key: string): boolean {
  try {
    return storageArea(kind)?.getItem(key) === '1'
  } catch {
    return false
  }
}

function persistFlag(kind: 'session' | 'local', key: string): void {
  try {
    storageArea(kind)?.setItem(key, '1')
  } catch {
    // Best-effort: the in-memory state still holds for this mount.
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
  const [lastRun, setLastRun] = useState<BrandMigrationRun | null>(null)
  const [dismissed, setDismissed] = useState<boolean>(() =>
    readFlag('session', DISMISS_STORAGE_KEY)
  )
  const [silenced, setSilenced] = useState<boolean>(() => readFlag('local', SILENCED_STORAGE_KEY))
  const [receipt, setReceipt] = useState<BrandMigrationReceipt | null>(null)
  const [running, setRunning] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void Promise.all([brandMigrationApi.detectLegacyData(), brandMigrationApi.lastRun()]).then(
      ([probe, run]) => {
        if (cancelled) return
        setDetection(probe)
        setLastRun(run)
      }
    )
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
    persistFlag('session', DISMISS_STORAGE_KEY)
    setDismissed(true)
  }, [])

  const silence = useCallback((): void => {
    persistFlag('local', SILENCED_STORAGE_KEY)
    setSilenced(true)
  }, [])

  const presentSignals = useMemo(
    () => detection?.signals.filter((signal) => signal.present) ?? [],
    [detection]
  )

  if (!detection) return null

  const sshFailure = detection.sshKnownHosts.state === 'failed' ? detection.sshKnownHosts : null
  // A recorded pass with nothing failed means the work is done, whatever the
  // probe says about roots still sitting on disk — they are meant to.
  const settled = lastRun !== null && !hasFailedRoots(lastRun)
  // The merge half of the banner. "Later" closes it for the run, "Don't show
  // again" for good, and a settled journal keeps it from ever opening.
  const showMergePrompt = detection.hasLegacyData && !settled && !dismissed && !silenced
  // Only offered once a pass has actually failed: before that, "Later" is the
  // right escape hatch and a permanent one would hide work the user still wants.
  const canSilence = hasFailedRoots(lastRun)

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

          {/* Where to find this again. Without it, "Later" reads as the last
              chance — which it no longer is, now that a settled journal keeps
              the banner from coming back on its own. */}
          <p data-testid="brand-migration-settings-hint" className="text-muted-foreground">
            {t('brandMigration.settingsHint')}
          </p>

          {error !== null && (
            <p role="alert" data-testid="brand-migration-error" className="text-destructive">
              {t('brandMigration.failedTitle')} — {t('brandMigration.reason', { reason: error })}
            </p>
          )}

          <div className="flex items-center justify-end gap-2 pt-1">
            {canSilence && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                data-testid="brand-migration-silence"
                onClick={silence}
              >
                {t('brandMigration.never')}
              </Button>
            )}
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
