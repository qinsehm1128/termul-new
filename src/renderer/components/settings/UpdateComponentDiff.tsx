import type { UpdateComponentPolicy } from '@shared/types/updater.types'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { logFrontendError } from '@/lib/log-api'
import {
  buildComponentDiff,
  type ComponentDiffRow,
  type ComponentRuntimeIdentities,
  fetchComponentRuntimeIdentities,
  shortBuildId
} from '@/lib/update-component-diff'
import { cn } from '@/lib/utils'

interface UpdateComponentDiffProps {
  policy: UpdateComponentPolicy
}

const STATUS_TONE: Record<ComponentDiffRow['status'], string> = {
  same: 'text-muted-foreground',
  replace: 'text-amber-500',
  bundled: 'text-muted-foreground',
  notRunning: 'text-muted-foreground'
}

/**
 * Running build IDs next to the downloaded release, one row per component, so
 * the user sees which parts an install replaces before choosing how to install.
 */
export function UpdateComponentDiff({ policy }: UpdateComponentDiffProps) {
  const { t } = useTranslation('shell')
  const [runtime, setRuntime] = useState<ComponentRuntimeIdentities | null>(null)
  const [error, setError] = useState<string | null>(null)

  // Read once per mount; the parent keys this component by release version.
  useEffect(() => {
    let cancelled = false
    fetchComponentRuntimeIdentities()
      .then((identities) => {
        if (!cancelled) setRuntime(identities)
      })
      .catch((reason: unknown) => {
        const message = reason instanceof Error ? reason.message : String(reason)
        void logFrontendError({
          level: 'warn',
          source: 'UpdateComponentDiff',
          message: `code=RUNTIME_IDENTITIES_UNAVAILABLE ${message}`
        })
        if (!cancelled) setError(message)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const rows = useMemo(
    () => (runtime ? buildComponentDiff(policy, runtime) : null),
    [policy, runtime]
  )

  return (
    <div>
      <label className="block text-sm font-medium text-secondary-foreground mb-2">
        {t('updates.componentDiff')}
      </label>
      <div className="rounded-md bg-secondary/25 px-3 py-2.5 text-xs">
        {error ? (
          <p className="text-red-500">{t('updates.componentDiffUnavailable', { error })}</p>
        ) : !rows ? (
          <p className="text-muted-foreground">{t('updates.componentDiffLoading')}</p>
        ) : (
          <table className="w-full table-fixed text-left" data-testid="update-component-diff">
            <thead className="text-muted-foreground">
              <tr>
                <th className="pb-1 font-medium">{t('updates.componentDiffComponent')}</th>
                <th className="pb-1 font-medium">{t('updates.componentDiffCurrent')}</th>
                <th className="pb-1 font-medium">{t('updates.componentDiffNext')}</th>
                <th className="pb-1 font-medium">{t('updates.componentDiffStatus')}</th>
              </tr>
            </thead>
            <tbody className="text-foreground">
              {rows.map((row) => (
                <tr key={row.component} data-component={row.component} data-status={row.status}>
                  <td className="py-0.5">{t(`updates.componentImpact.${row.component}`)}</td>
                  <td className="py-0.5 font-mono" title={row.currentBuildId ?? undefined}>
                    {row.currentBuildId ? shortBuildId(row.currentBuildId) : '—'}
                  </td>
                  <td className="py-0.5 font-mono" title={row.nextBuildId}>
                    {shortBuildId(row.nextBuildId)}
                  </td>
                  <td className={cn('py-0.5', STATUS_TONE[row.status])}>
                    {t(`updates.componentDiffStatuses.${row.status}`)}
                    {row.activeResources !== null && row.activeResources > 0 && (
                      <span className="block text-muted-foreground">
                        {row.component === 'terminalCore'
                          ? t('updates.activeTerminals', { count: row.activeResources })
                          : t('updates.activeAgentSessions', { count: row.activeResources })}
                      </span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}
