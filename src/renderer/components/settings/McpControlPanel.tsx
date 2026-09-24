import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { McpServersSettings } from '@/components/settings/McpServersSettings'
import { MemoryMcpConfigSection } from '@/components/settings/MemoryMcpConfigSection'
import { Switch } from '@/components/ui/switch'
import { BUILTIN_PROJECT_SCOPE, BUILTIN_SESSION_MEMORY } from '@/lib/mcp-api'
import { useMcpStore } from '@/stores/mcp-store'

const BUILTIN_ORDER = [BUILTIN_SESSION_MEMORY, BUILTIN_PROJECT_SCOPE] as const

export function McpControlPanel(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const builtIns = useMcpStore((state) => state.config.builtIns)
  const revision = useMcpStore((state) => state.config.revision)
  const status = useMcpStore((state) => state.status)
  const loadError = useMcpStore((state) => state.loadError)
  const saveError = useMcpStore((state) => state.saveError)
  const setBuiltInEnabled = useMcpStore((state) => state.setBuiltInEnabled)
  const loadStatus = useMcpStore((state) => state.loadStatus)

  // The status panel is intentionally refreshed after each successful config revision.
  // biome-ignore lint/correctness/useExhaustiveDependencies: revision triggers the status refresh.
  useEffect(() => {
    void loadStatus()
  }, [loadStatus, revision])

  const orderedBuiltIns = [...builtIns].sort((left, right) => {
    const leftIndex = BUILTIN_ORDER.indexOf(left.id as (typeof BUILTIN_ORDER)[number])
    const rightIndex = BUILTIN_ORDER.indexOf(right.id as (typeof BUILTIN_ORDER)[number])
    return (leftIndex === -1 ? 99 : leftIndex) - (rightIndex === -1 ? 99 : rightIndex)
  })

  return (
    <div className="space-y-4">
      {(loadError || saveError) && (
        <p className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {saveError ?? loadError}
        </p>
      )}

      <section className="space-y-3 rounded-lg border border-border bg-secondary/20 p-4">
        <div>
          <p className="text-sm font-medium text-foreground">{t('page.builtInsTitle')}</p>
          <p className="text-xs text-muted-foreground">{t('page.builtInsDescription')}</p>
        </div>
        <ul className="space-y-2">
          {orderedBuiltIns.map((item) => (
            <li key={item.id} className="flex items-center justify-between gap-3">
              <div className="min-w-0">
                <p className="text-sm text-foreground">
                  {t(`page.builtIn.${item.id}.name`, { defaultValue: item.id })}
                </p>
                <p className="text-xs text-muted-foreground">
                  {t(`page.builtIn.${item.id}.description`, { defaultValue: item.id })}
                </p>
              </div>
              <Switch
                checked={item.enabled}
                aria-label={t(item.enabled ? 'common.disable' : 'common.enable', {
                  name: t(`page.builtIn.${item.id}.name`, { defaultValue: item.id })
                })}
                onCheckedChange={(enabled) => {
                  void setBuiltInEnabled(item.id, enabled)
                }}
              />
            </li>
          ))}
        </ul>
      </section>

      <McpServersSettings />

      <section className="space-y-2 rounded-lg border border-border bg-secondary/20 p-4">
        <p className="text-sm font-medium text-foreground">{t('page.statusTitle')}</p>
        <p className="text-xs text-muted-foreground">{t('page.statusDescription')}</p>
        <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
          <dt className="text-muted-foreground">{t('page.revision')}</dt>
          <dd className="font-mono text-foreground">{status?.revision ?? revision}</dd>
          <dt className="text-muted-foreground">{t('page.upstreamCount')}</dt>
          <dd className="font-mono text-foreground">{status?.upstreams.length ?? 0}</dd>
        </dl>
      </section>

      <MemoryMcpConfigSection />
    </div>
  )
}
