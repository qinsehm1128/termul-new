import type { HostStatusRecord } from '@shared/types/plugin-host.types'
import { useTranslation } from 'react-i18next'
import { cn } from '@/lib/utils'
import { usePluginsStore } from '@/stores/plugins-store'
import { usePluginHostUiStore } from './host-ui-store'
import { getPluginViewIcon } from './view-icons'

export interface PluginStatusItemsProps {
  projectId?: string
}

const toneClasses = {
  default: 'text-status-bar-foreground',
  muted: 'text-muted-foreground',
  success: 'text-success',
  warning: 'text-warning',
  danger: 'text-destructive'
} as const

function inScope(item: HostStatusRecord, projectId?: string): boolean {
  return (
    item.visible !== false &&
    (item.expiresAt === null || item.expiresAt > Date.now()) &&
    (item.scope === 'window' || (Boolean(projectId) && item.projectId === projectId))
  )
}

export function PluginStatusItems({ projectId }: PluginStatusItemsProps): React.JSX.Element | null {
  const { t } = useTranslation('plugins')
  const snapshot = usePluginHostUiStore((state) => state.snapshot)
  const contextReady = usePluginHostUiStore(
    (state) =>
      state.connected &&
      !state.contextPending &&
      state.context !== null &&
      state.contextRevision > 0
  )
  const connected = usePluginsStore((state) => state.connected)
  const plugins = usePluginsStore((state) => state.plugins)
  const bootId = usePluginsStore((state) => state.bootId)
  if (!connected || !snapshot || snapshot.bootId !== bootId) return null

  const items = snapshot.statusItems
    .filter(
      (item) =>
        inScope(item, projectId) &&
        plugins.some(
          (plugin) =>
            plugin.id === item.pluginId &&
            plugin.state === 'active' &&
            plugin.desiredEnabled &&
            plugin.generation === item.generation
        )
    )
    .sort(
      (a, b) =>
        a.order - b.order ||
        (a.pluginId < b.pluginId ? -1 : a.pluginId > b.pluginId ? 1 : 0) ||
        (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
    )
  if (!items.length) return null

  const execute = (item: HostStatusRecord): void => {
    const {
      snapshot,
      execute,
      contextRevision,
      connected: hostConnected,
      contextPending,
      context
    } = usePluginHostUiStore.getState()
    const { connected, plugins, bootId } = usePluginsStore.getState()
    // Recheck the lease when a cached DOM handler runs after a lifecycle change.
    if (
      !connected ||
      !hostConnected ||
      contextPending ||
      !context ||
      contextRevision <= 0 ||
      (projectId !== undefined && context.activeProject?.id !== projectId) ||
      snapshot?.bootId !== bootId ||
      !inScope(item, projectId) ||
      !snapshot.statusItems.some(
        (current) =>
          current.pluginId === item.pluginId &&
          current.generation === item.generation &&
          current.id === item.id &&
          current.revision === item.revision
      ) ||
      !plugins.some(
        (plugin) =>
          plugin.id === item.pluginId &&
          plugin.desiredEnabled &&
          plugin.state === 'active' &&
          plugin.generation === item.generation
      )
    )
      return
    void execute({
      pluginId: item.pluginId,
      generation: item.generation,
      kind: 'status',
      contributionId: item.id,
      contextRevision,
      target: {
        ...(context.activeProject ? { projectId: context.activeProject.id } : {}),
        ...(context.activeTerminalId ? { terminalId: context.activeTerminalId } : {}),
        ...(context.activeSessionId ? { sessionId: context.activeSessionId } : {})
      }
    }).catch(() => undefined)
  }

  return (
    <fieldset className="flex min-w-0 items-center gap-1 border-0 p-0">
      <legend className="sr-only">{t('status.items')}</legend>
      {items.map((item) => {
        const Icon = getPluginViewIcon(item.icon)
        const label = `${item.pluginName} (${item.pluginId}): ${item.label} — ${item.text}`
        const className = cn(
          'flex h-5 min-w-0 items-center gap-1 rounded-sm px-1.5',
          item.tone && Object.hasOwn(toneClasses, item.tone)
            ? toneClasses[item.tone]
            : toneClasses.default
        )
        const content = (
          <>
            <Icon size={14} aria-hidden="true" className="shrink-0" />
            <span className="max-w-48 truncate">{item.text}</span>
          </>
        )
        return item.commandId ? (
          <button
            key={`${item.pluginId}/${item.generation}/${item.id}`}
            type="button"
            aria-label={label}
            title={`${label}${item.tooltip ? `\n${item.tooltip}` : ''}`}
            className={cn(
              className,
              'hover:bg-foreground/[0.045] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring'
            )}
            disabled={!contextReady}
            onClick={() => execute(item)}
          >
            {content}
          </button>
        ) : (
          <span
            key={`${item.pluginId}/${item.generation}/${item.id}`}
            title={`${label}${item.tooltip ? `\n${item.tooltip}` : ''}`}
            className={className}
          >
            <span className="sr-only">
              {item.pluginName} ({item.pluginId}): {item.label} —{' '}
            </span>
            {content}
          </span>
        )
      })}
    </fieldset>
  )
}
