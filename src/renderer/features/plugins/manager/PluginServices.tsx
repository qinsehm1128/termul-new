import type { PluginInfo, PluginService } from '@shared/types/plugin.types'
import { isAvailableHostService, isReservedHostService } from '@shared/types/plugin-host.types'
import { useTranslation } from 'react-i18next'
import { Badge } from '@/components/ui/badge'

export interface PluginServicesProps {
  plugin: PluginInfo
  plugins: PluginInfo[]
  connected: boolean
}

/** Reflects live availability, not merely the presence of a manifest declaration. */
export function PluginServices({
  plugin,
  plugins,
  connected
}: PluginServicesProps): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const active = (entry: PluginInfo) => entry.state === 'active' && entry.desiredEnabled
  const requirementAvailable = (service: PluginService): boolean =>
    connected &&
    (isReservedHostService(service.name)
      ? isAvailableHostService(service.name, service.version)
      : plugins.some(
          (peer) =>
            peer.id !== plugin.id &&
            active(peer) &&
            peer.manifest.services.provides.some(
              (provided) => provided.name === service.name && provided.version === service.version
            )
        ))

  return (
    <div className="space-y-5">
      {(['provides', 'requires'] as const).map((kind) => {
        const services = plugin.manifest.services[kind]
        const title = t(kind === 'provides' ? 'inspector.provides' : 'inspector.requires')
        return (
          <section key={kind} aria-label={title} className="space-y-2">
            <h3 className="text-xs font-medium text-foreground">{title}</h3>
            {services.length ? (
              <ul className="divide-y divide-border/60 text-xs">
                {services.map((service) => {
                  const available =
                    kind === 'provides'
                      ? connected && active(plugin) && !isReservedHostService(service.name)
                      : requirementAvailable(service)
                  return (
                    <li
                      key={`${service.name}:${service.version}`}
                      className="flex flex-wrap items-center gap-2 py-2 first:pt-0 last:pb-0"
                    >
                      <span className="min-w-0 break-all font-medium">{service.name}</span>
                      <Badge variant="outline">v{service.version}</Badge>
                      <Badge variant={available ? 'secondary' : 'outline'}>
                        {t(available ? 'inspector.available' : 'inspector.unavailable')}
                      </Badge>
                    </li>
                  )
                })}
              </ul>
            ) : (
              <p className="text-xs text-muted-foreground">
                {t(kind === 'provides' ? 'inspector.emptyProvides' : 'inspector.emptyRequires')}
              </p>
            )}
          </section>
        )
      })}
      <p className="text-xs text-muted-foreground">{t('inspector.secretsUnavailable')}</p>
    </div>
  )
}
