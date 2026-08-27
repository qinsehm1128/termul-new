import {
  AlertTriangle,
  CheckCircle2,
  CircleSlash,
  ExternalLink,
  HelpCircle,
  Loader2,
  MinusCircle,
  RefreshCw
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { logFrontendError } from '@/lib/log-api'
import {
  fetchPermissionReport,
  openPrivacyPane,
  type PermissionId,
  type PermissionProbe,
  type PermissionReport,
  type PermissionState
} from '@/lib/macos-permissions-api'
import { cn } from '@/lib/utils'

/**
 * Display order, most consequential first.
 *
 * Deliberately fixed rather than sorted by state: a list that reshuffles as
 * probes resolve makes the row under the cursor move out from under it.
 */
const DISPLAY_ORDER = [
  'localNetwork',
  'fullDiskAccess',
  'accessibility',
  'screenRecording',
  'inputMonitoring',
  'desktopFolder',
  'documentsFolder',
  'downloadsFolder'
] as const satisfies readonly PermissionId[]

/**
 * Compile-time proof that the order above covers every id.
 *
 * `orderProbes` drops anything it cannot place, so an id added to
 * `PermissionId` but not here would silently disappear from the panel. This
 * turns that into a type error naming the missing id.
 */
type UnplacedIds = Exclude<PermissionId, (typeof DISPLAY_ORDER)[number]>
const _everyIdIsDisplayed: [UnplacedIds] extends [never] ? true : UnplacedIds = true
void _everyIdIsDisplayed

const STATE_STYLES: Record<PermissionState, { icon: React.ReactNode; className: string }> = {
  granted: {
    icon: <CheckCircle2 size={14} />,
    className: 'bg-green-500/10 text-green-500 ring-green-500/20'
  },
  // Amber, not red: for the preflight probes "denied" and "never asked" are the
  // same return value, so this is a prompt to look, not a failure.
  denied: {
    icon: <AlertTriangle size={14} />,
    className: 'bg-amber-500/10 text-amber-500 ring-amber-500/20'
  },
  unknown: {
    icon: <HelpCircle size={14} />,
    className: 'bg-secondary/50 text-muted-foreground ring-border/70'
  },
  notProbed: {
    icon: <CircleSlash size={14} />,
    className: 'bg-secondary/50 text-muted-foreground ring-border/70'
  },
  notRequired: {
    icon: <MinusCircle size={14} />,
    className: 'bg-secondary/50 text-muted-foreground ring-border/70'
  }
}

function orderProbes(probes: readonly PermissionProbe[]): PermissionProbe[] {
  const byId = new Map(probes.map((probe) => [probe.id, probe]))
  return DISPLAY_ORDER.map((id) => byId.get(id)).filter(
    (probe): probe is PermissionProbe => probe !== undefined
  )
}

/**
 * macOS privacy grants, as this app actually experiences them.
 *
 * Two kinds of row live here and the difference is load-bearing. Preflight rows
 * are filled in the moment the panel opens because asking costs nothing. Rows
 * marked `active` are left at `notProbed` until the user presses Check, because
 * the only way to learn those answers is to attempt the operation — which is
 * also what makes macOS show its prompt.
 */
export function MacosPermissionsSettings(): React.JSX.Element {
  const { t } = useTranslation('settings')
  const [report, setReport] = useState<PermissionReport | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [pending, setPending] = useState<PermissionId | 'all' | null>('all')
  // Ids the user has already consented to probe. Replayed on every refresh so a
  // second Check does not reset the first row back to "not probed".
  const consented = useRef<Set<PermissionId>>(new Set())
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const refresh = useCallback(async (probe: PermissionId | null): Promise<void> => {
    if (probe) consented.current.add(probe)
    setPending(probe ?? 'all')
    setError(null)
    try {
      const next = await fetchPermissionReport([...consented.current])
      if (mounted.current) setReport(next)
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause)
      if (mounted.current) setError(message)
      void logFrontendError({ source: 'MacosPermissionsSettings', message })
    } finally {
      if (mounted.current) setPending(null)
    }
  }, [])

  useEffect(() => {
    void refresh(null)
  }, [refresh])

  const probes = useMemo(() => orderProbes(report?.probes ?? []), [report])
  const signing = report?.signing ?? null

  return (
    <div className="w-2/3 space-y-4">
      {/* Identity first: it explains why a grant the user already gave can be
          gone again after an update, which no per-row state can convey. */}
      <div className="rounded-md bg-secondary/25 px-3 py-2.5 shadow-[inset_0_1px_0_0_hsl(var(--foreground)/0.035)]">
        <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1 text-xs">
          <span className="text-muted-foreground">
            {t('macosPrivacy.osVersion')}
            <span className="ml-1.5 font-mono text-foreground">{report?.osVersion ?? '—'}</span>
          </span>
          <span className="text-muted-foreground">
            {t('macosPrivacy.bundleId')}
            <span className="ml-1.5 font-mono text-foreground">{report?.bundleId ?? '—'}</span>
          </span>
          <span className="text-muted-foreground">
            {t('macosPrivacy.signature')}
            <span className="ml-1.5 font-mono text-foreground">
              {signing ? t(`macosPrivacy.signingKind.${signing.kind}`) : '—'}
              {signing?.teamId ? ` (${signing.teamId})` : ''}
            </span>
          </span>
        </div>
        {signing && !signing.grantsSurviveRebuild && (
          <div className="mt-2 flex items-start gap-2 rounded-md border border-amber-500/20 bg-amber-500/10 px-2.5 py-2">
            <AlertTriangle size={14} className="mt-0.5 flex-shrink-0 text-amber-500" />
            <p className="text-xs text-foreground">{t('macosPrivacy.unstableIdentity')}</p>
          </div>
        )}
      </div>

      {error && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-md border border-red-500/20 bg-red-500/10 px-3 py-2.5"
        >
          <AlertTriangle size={14} className="mt-0.5 flex-shrink-0 text-red-500" />
          <p className="text-xs text-foreground">{error}</p>
        </div>
      )}

      <div className="space-y-2">
        {probes.map((probe) => {
          const style = STATE_STYLES[probe.state] ?? STATE_STYLES.unknown
          const isPending = pending === probe.id || pending === 'all'
          return (
            <div
              key={probe.id}
              data-permission-row={probe.id}
              data-permission-state={probe.state}
              className="rounded-md bg-secondary/25 px-3 py-2.5 shadow-[inset_0_1px_0_0_hsl(var(--foreground)/0.035)]"
            >
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-sm text-foreground">
                      {t(`macosPrivacy.items.${probe.id}.label`, probe.id)}
                    </span>
                    <span
                      className={cn(
                        'inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-3xs ring-1 ring-inset',
                        style.className
                      )}
                    >
                      {style.icon}
                      {t(`macosPrivacy.state.${probe.state}`, probe.state)}
                    </span>
                  </div>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    {t(`macosPrivacy.items.${probe.id}.hint`, '')}
                  </p>
                  {probe.detail && (
                    <p className="mt-1 font-mono text-3xs text-muted-foreground/70">
                      {probe.detail}
                    </p>
                  )}
                </div>
                <div className="flex flex-shrink-0 items-center gap-1.5">
                  {probe.active && (
                    <button
                      type="button"
                      onClick={() => void refresh(probe.id)}
                      disabled={isPending}
                      title={t('macosPrivacy.checkHint')}
                      className="inline-flex h-7 items-center gap-1.5 rounded-md bg-secondary/60 px-2.5 text-xs text-foreground transition-colors duration-150 hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      {isPending ? (
                        <Loader2 size={12} className="animate-spin" />
                      ) : (
                        <RefreshCw size={12} />
                      )}
                      {t('macosPrivacy.check')}
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => void openPrivacyPane(probe.id)}
                    className="inline-flex h-7 items-center gap-1.5 rounded-md bg-secondary/60 px-2.5 text-xs text-foreground transition-colors duration-150 hover:bg-secondary"
                  >
                    <ExternalLink size={12} />
                    {t('macosPrivacy.openSettings')}
                  </button>
                </div>
              </div>
            </div>
          )
        })}
      </div>

      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => void refresh(null)}
          disabled={pending !== null}
          className="inline-flex h-8 items-center gap-2 rounded-md bg-secondary/50 px-3 text-sm text-foreground transition-colors duration-150 hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-50"
        >
          {pending !== null ? (
            <Loader2 size={14} className="animate-spin" />
          ) : (
            <RefreshCw size={14} />
          )}
          {t('macosPrivacy.recheck')}
        </button>
      </div>

      <p className="text-xs text-muted-foreground">{t('macosPrivacy.deniedCaveat')}</p>
    </div>
  )
}
