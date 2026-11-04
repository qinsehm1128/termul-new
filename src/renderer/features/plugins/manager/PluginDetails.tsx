import type { PluginInfo } from '@shared/types/plugin.types'
import { RefreshCw, ShieldAlert, Trash2, Upload } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { PluginSettingsForm } from '@/features/plugins/PluginSettingsForm'
import { getPluginViewIcon } from '@/features/plugins/view-icons'
import { PluginContributions } from './PluginContributions'
import { lifecyclePending, PluginStatus } from './PluginList'
import { PluginProcesses } from './PluginProcesses'
import { PluginServices } from './PluginServices'

interface PluginDetailsProps {
  plugin: PluginInfo
  plugins: PluginInfo[]
  connected: boolean
  disabled: boolean
  onRetry: () => void
  onReload: () => void
  onUpgrade: () => void
  onUninstall: () => void
}

export function PluginDetails({
  plugin,
  plugins,
  connected,
  disabled,
  onRetry,
  onReload,
  onUpgrade,
  onUninstall
}: PluginDetailsProps): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const [tab, setTab] = useState('overview')
  const [settingsVisited, setSettingsVisited] = useState(false)
  const Icon = getPluginViewIcon(
    plugin.manifest.contributes.views.find((view) => view.location === 'sidebar')?.icon
  )
  const unavailable = disabled || lifecyclePending(plugin)
  const metadata = [
    [t('manager.identifier'), plugin.id],
    [t('manager.version'), plugin.manifest.version],
    [t('manager.abi'), `${plugin.manifest.abi.min}–${plugin.manifest.abi.max}`],
    [t('manager.packagePath'), t('manager.notExposed')],
    ['SHA-256', plugin.sha256]
  ]
  return (
    <section aria-label={t('manager.details')} className="min-w-0 flex-1">
      <div className="flex items-start gap-3 border-b border-border p-5">
        <span className="flex size-11 shrink-0 items-center justify-center rounded-lg border border-border bg-muted/40 text-primary">
          <Icon size={22} aria-hidden="true" />
        </span>
        <div className="min-w-0 space-y-1.5">
          <h2 className="break-words text-base font-medium">{plugin.manifest.name}</h2>
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-xs text-muted-foreground">
              {t('manager.version')} {plugin.manifest.version}
            </span>
            <PluginStatus plugin={plugin} />
          </div>
        </div>
      </div>
      <Tabs
        value={tab}
        onValueChange={(value) => {
          setTab(value)
          if (value === 'settings') setSettingsVisited(true)
        }}
        className="p-5"
      >
        <TabsList aria-label={t('manager.detailTabs')} className="h-auto flex-wrap justify-start">
          {(['overview', 'settings', 'contributions', 'services', 'processes'] as const).map(
            (tab) => (
              <TabsTrigger key={tab} value={tab}>
                {t(`manager.tabs.${tab}`)}
              </TabsTrigger>
            )
          )}
        </TabsList>
        <TabsContent value="overview" className="space-y-5 pt-3">
          <p className="text-sm leading-relaxed text-muted-foreground">
            {t('manager.noDescription')}
          </p>
          {(plugin.state === 'quarantined' || plugin.state === 'failed' || plugin.lastError) && (
            <div
              role="alert"
              className="space-y-3 rounded-md border border-destructive/30 bg-destructive/5 p-4"
            >
              {plugin.state === 'quarantined' && (
                <p className="text-sm font-medium text-destructive">
                  {t('manager.quarantineWarning')}
                </p>
              )}
              {plugin.lastError && (
                <p className="break-words text-sm text-destructive">
                  {t('manager.lastError')}: {plugin.lastError.code}: {plugin.lastError.message}
                </p>
              )}
              {(plugin.state === 'quarantined' || plugin.state === 'failed') && (
                <Button variant="outline" size="sm" disabled={unavailable} onClick={onRetry}>
                  <RefreshCw size={14} aria-hidden="true" />
                  {t('manager.retry')}
                </Button>
              )}
            </div>
          )}
          <dl className="divide-y divide-border/60 rounded-md border border-border px-4 text-sm">
            {metadata.map(([label, value]) => (
              <div
                key={label}
                className="grid gap-1 py-3 sm:grid-cols-[7rem_minmax(0,1fr)] sm:gap-4"
              >
                <dt className="text-muted-foreground">{label}</dt>
                <dd className="break-all font-mono text-xs leading-5">{value}</dd>
              </div>
            ))}
          </dl>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={unavailable || plugin.state === 'quarantined'}
              onClick={onReload}
            >
              <RefreshCw size={14} aria-hidden="true" />
              {t('manager.reload')}
            </Button>
            <Button variant="outline" size="sm" disabled={unavailable} onClick={onUpgrade}>
              <Upload size={14} aria-hidden="true" />
              {t('manager.upgrade')}
            </Button>
            <Button variant="destructive" size="sm" disabled={unavailable} onClick={onUninstall}>
              <Trash2 size={14} aria-hidden="true" />
              {t('manager.uninstall')}
            </Button>
          </div>
        </TabsContent>
        <TabsContent
          value="settings"
          forceMount={settingsVisited ? true : undefined}
          hidden={tab !== 'settings'}
          className="pt-3"
        >
          {connected && plugin.state === 'active' ? (
            <fieldset disabled={disabled} className="min-w-0">
              <legend className="sr-only">{t('settings.title')}</legend>
              <PluginSettingsForm pluginId={plugin.id} />
            </fieldset>
          ) : (
            <p className="text-sm text-muted-foreground">{t('manager.settingsUnavailable')}</p>
          )}
        </TabsContent>
        <TabsContent value="contributions" className="pt-3">
          <PluginContributions plugin={plugin} connected={connected} />
        </TabsContent>
        <TabsContent value="services" className="pt-3">
          <PluginServices plugin={plugin} plugins={plugins} connected={connected} />
        </TabsContent>
        <TabsContent value="processes" className="mt-6">
          <PluginProcesses
            plugin={plugin}
            connected={connected}
            disabled={disabled || lifecyclePending(plugin)}
          />
        </TabsContent>
      </Tabs>
      <div className="mx-5 mb-5 flex items-start gap-2 rounded-md border border-border bg-muted/30 p-3 text-xs leading-relaxed text-muted-foreground">
        <ShieldAlert size={16} className="mt-0.5 shrink-0" aria-hidden="true" />
        <p>{t('manager.trustWarning')}</p>
      </div>
    </section>
  )
}
