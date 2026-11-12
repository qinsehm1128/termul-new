import type { PluginInfo } from '@shared/types/plugin.types'
import { useTranslation } from 'react-i18next'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { getPluginViewIcon } from '@/features/plugins/view-icons'
import { cn } from '@/lib/utils'

export function lifecyclePending(plugin: PluginInfo): boolean {
  return ['pending', 'activating', 'reloading'].includes(plugin.state)
}

export function PluginStatus({ plugin }: { plugin: PluginInfo }): React.JSX.Element {
  const { t } = useTranslation('plugins')
  return (
    <Badge variant={['quarantined', 'failed'].includes(plugin.state) ? 'destructive' : 'secondary'}>
      {t(`state.${plugin.state}`)}
    </Badge>
  )
}

interface PluginListProps {
  plugins: PluginInfo[]
  selectedId?: string
  disabled: boolean
  /** The browser/phone client can open views but cannot change host state. */
  manageable?: boolean
  onSelect: (id: string) => void
  onToggle: (id: string, enabled: boolean) => void
}

export function PluginList({
  plugins,
  selectedId,
  disabled,
  manageable = true,
  onSelect,
  onToggle
}: PluginListProps): React.JSX.Element {
  const { t } = useTranslation('plugins')
  return (
    <ul aria-label={t('manager.installed')} className="divide-y divide-border/60">
      {plugins.map((plugin) => {
        const Icon = getPluginViewIcon(
          plugin.manifest.contributes.views.find((view) => view.location === 'sidebar')?.icon
        )
        const selected = selectedId === plugin.id
        return (
          <li
            key={plugin.id}
            className={cn('flex items-center gap-3 pr-4', selected && 'bg-accent/60')}
          >
            <button
              type="button"
              aria-label={plugin.manifest.name}
              aria-pressed={selected}
              onClick={() => onSelect(plugin.id)}
              className={cn(
                'flex min-w-0 flex-1 items-start gap-3 border-l-2 p-4 text-left transition-colors hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring',
                selected ? 'border-primary' : 'border-transparent'
              )}
            >
              <span className="flex size-9 shrink-0 items-center justify-center rounded-md border border-border bg-background text-muted-foreground">
                <Icon size={18} aria-hidden="true" />
              </span>
              <span className="min-w-0 space-y-1.5">
                <span className="block truncate text-sm font-medium text-foreground">
                  {plugin.manifest.name}
                </span>
                <span className="flex flex-wrap items-center gap-2">
                  <span className="text-xs text-muted-foreground">{plugin.manifest.version}</span>
                  <PluginStatus plugin={plugin} />
                </span>
              </span>
            </button>
            {manageable ? (
              <Switch
                checked={plugin.desiredEnabled}
                disabled={disabled || lifecyclePending(plugin)}
                aria-label={`${t(plugin.desiredEnabled ? 'manager.disable' : 'manager.enable')} ${plugin.manifest.name}`}
                onCheckedChange={(enabled) => onToggle(plugin.id, enabled)}
              />
            ) : null}
          </li>
        )
      })}
    </ul>
  )
}
