import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { McpHostAccessPanel } from '@/components/settings/McpHostAccessPanel'
import { McpServersSettings } from '@/components/settings/McpServersSettings'
import { Switch } from '@/components/ui/switch'
import {
  BUILTIN_PROJECT_SCOPE,
  BUILTIN_SESSION_MEMORY,
  BUILTIN_SSH,
  BUILTIN_WORKSPACE
} from '@/lib/mcp-api'
import { useMcpStore } from '@/stores/mcp-store'

const BUILTIN_ORDER = [BUILTIN_SESSION_MEMORY, BUILTIN_WORKSPACE, BUILTIN_SSH] as const

function McpStoreError(): React.JSX.Element | null {
  const loadError = useMcpStore((state) => state.loadError)
  const saveError = useMcpStore((state) => state.saveError)
  if (!loadError && !saveError) return null
  return (
    <p
      role="alert"
      className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
    >
      {saveError ?? loadError}
    </p>
  )
}

/** "External servers" tab: only the upstream registry the aggregator serves. */
export function McpUpstreamsPanel(): React.JSX.Element {
  const revision = useMcpStore((state) => state.config.revision)
  const loadStatus = useMcpStore((state) => state.loadStatus)

  // The status is intentionally refreshed after each successful config revision.
  // biome-ignore lint/correctness/useExhaustiveDependencies: revision triggers the status refresh.
  useEffect(() => {
    void loadStatus()
  }, [loadStatus, revision])

  return (
    <div className="space-y-4">
      <McpStoreError />
      <McpServersSettings />
    </div>
  )
}

/** "Se capabilities" tab: built-in switches first, then who may use them. */
export function McpCapabilitiesPanel(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const builtIns = useMcpStore((state) => state.config.builtIns)
  const setBuiltInEnabled = useMcpStore((state) => state.setBuiltInEnabled)

  const orderedBuiltIns = builtIns
    .filter((item) => item.id !== BUILTIN_PROJECT_SCOPE)
    .sort((left, right) => {
      const leftIndex = BUILTIN_ORDER.indexOf(left.id as (typeof BUILTIN_ORDER)[number])
      const rightIndex = BUILTIN_ORDER.indexOf(right.id as (typeof BUILTIN_ORDER)[number])
      return (leftIndex === -1 ? 99 : leftIndex) - (rightIndex === -1 ? 99 : rightIndex)
    })

  return (
    <div className="space-y-6">
      <McpStoreError />
      <section aria-labelledby="mcp-builtins-title" className="space-y-3">
        <div>
          <h2 id="mcp-builtins-title" className="text-sm font-semibold text-foreground">
            {t('page.builtInsTitle')}
          </h2>
          <p className="mt-0.5 text-xs text-muted-foreground">{t('page.builtInsDescription')}</p>
        </div>
        <ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {orderedBuiltIns.map((item) => {
            const name = t(`page.builtIn.${item.id}.name`, { defaultValue: item.id })
            return (
              <li
                key={item.id}
                className="flex items-start justify-between gap-3 rounded-lg border border-border bg-card/40 p-3"
              >
                <div className="min-w-0">
                  <p className="text-sm font-medium text-foreground">{name}</p>
                  <p className="mt-0.5 text-xs leading-relaxed text-muted-foreground">
                    {t(`page.builtIn.${item.id}.description`, { defaultValue: item.id })}
                  </p>
                </div>
                <Switch
                  checked={item.enabled}
                  aria-label={t(item.enabled ? 'common.disable' : 'common.enable', { name })}
                  onCheckedChange={(enabled) => {
                    void setBuiltInEnabled(item.id, enabled)
                  }}
                />
              </li>
            )
          })}
        </ul>
      </section>
      <McpHostAccessPanel />
    </div>
  )
}
