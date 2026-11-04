import type { PluginInfo } from '@shared/types/plugin.types'
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { isMac } from '@/lib/platform'
import { pluginShortcutId, useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'

export interface PluginContributionsProps {
  plugin: PluginInfo
  connected: boolean
}

function ContributionSection({
  title,
  empty,
  count,
  children
}: {
  title: string
  empty: string
  count: number
  children: ReactNode
}): React.JSX.Element {
  return (
    <section aria-label={title} className="space-y-2">
      <h3 className="text-xs font-medium text-foreground">{title}</h3>
      {count ? (
        <ul className="divide-y divide-border/60 text-xs">{children}</ul>
      ) : (
        <p className="text-xs text-muted-foreground">{empty}</p>
      )}
    </section>
  )
}

function Detail({ label, children }: { label: string; children: ReactNode }): React.JSX.Element {
  return (
    <span className="inline-flex min-w-0 gap-1">
      <span className="text-muted-foreground">{label}:</span>
      <span className="break-all">{children}</span>
    </span>
  )
}

/** Manifest metadata only; view navigation is the sole executable affordance. */
export function PluginContributions({
  plugin,
  connected
}: PluginContributionsProps): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const shortcuts = useKeyboardShortcutsStore((state) => state.shortcuts)
  const overrides = useKeyboardShortcutsStore((state) => state.pluginOverrides)
  const conflicts = useKeyboardShortcutsStore((state) => state.pluginConflicts)
  const {
    views,
    commands,
    menus = [],
    keybindings = [],
    statusItems = [],
    eventSubscriptions = []
  } = plugin.manifest.contributes
  const executable =
    connected && plugin.state === 'active' && plugin.desiredEnabled && !!plugin.generation
  const rowClass = 'space-y-1 py-2 first:pt-0 last:pb-0'
  const detailsClass = 'flex flex-wrap gap-x-3 gap-y-1 text-xs'

  return (
    <div className="space-y-5">
      <ContributionSection
        title={t('inspector.views')}
        empty={t('inspector.emptyViews')}
        count={views.length}
      >
        {views.map((view) => (
          <li key={view.id} className={rowClass}>
            {executable ? (
              <Button asChild variant="outline" size="sm">
                <Link
                  to={`/plugins/${encodeURIComponent(plugin.id)}/views/${encodeURIComponent(view.id)}`}
                >
                  {view.title}
                </Link>
              </Button>
            ) : (
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled
                title={t('inspector.viewUnavailable')}
              >
                {view.title}
              </Button>
            )}
            <div className={detailsClass}>
              <Detail label={t('inspector.id')}>{view.id}</Detail>
              <Detail label={t('inspector.location')}>{view.location}</Detail>
            </div>
          </li>
        ))}
      </ContributionSection>
      <ContributionSection
        title={t('inspector.commands')}
        empty={t('inspector.emptyCommands')}
        count={commands.length}
      >
        {commands.map((command) => (
          <li key={command.id} className={rowClass}>
            <p className="break-words font-medium">{command.title}</p>
            <Detail label={t('inspector.id')}>{command.id}</Detail>
            {command.description && (
              <p className="break-words text-muted-foreground">{command.description}</p>
            )}
          </li>
        ))}
      </ContributionSection>
      <ContributionSection
        title={t('inspector.menus')}
        empty={t('inspector.emptyMenus')}
        count={menus.length}
      >
        {menus.map((menu) => (
          <li key={menu.id} className={`${rowClass} ${detailsClass}`}>
            <Detail label={t('inspector.id')}>{menu.id}</Detail>
            <Detail label={t('inspector.commandId')}>{menu.commandId}</Detail>
            <Detail label={t('inspector.location')}>{menu.location}</Detail>
          </li>
        ))}
      </ContributionSection>
      <ContributionSection
        title={t('inspector.keybindings')}
        empty={t('inspector.emptyKeybindings')}
        count={keybindings.length}
      >
        {keybindings.map((binding) => {
          const id = pluginShortcutId(plugin.id, binding.id)
          const shortcut = shortcuts[id]
          const defaultKey =
            shortcut?.defaultKey ?? (isMac ? (binding.mac ?? binding.key) : binding.key)
          const customKey = overrides[id]
          // The resolved store may deliberately unbind a conflicting key. Never re-enable it here.
          const effectiveKey =
            shortcut?.customKey ?? shortcut?.defaultKey ?? customKey ?? defaultKey
          return (
            <li key={binding.id} className={rowClass}>
              <div className="flex flex-wrap items-center gap-2">
                <span className="break-all font-medium">{binding.id}</span>
                <Badge variant="outline">{effectiveKey || t('inspector.unbound')}</Badge>
              </div>
              <div className={detailsClass}>
                <Detail label={t('inspector.commandId')}>{binding.commandId}</Detail>
                <Detail label={t('inspector.defaultKey')}>{defaultKey}</Detail>
                {customKey !== undefined && (
                  <Detail label={t('inspector.customKey')}>
                    {customKey || t('inspector.unbound')}
                  </Detail>
                )}
              </div>
              {(conflicts[id] ?? []).map((conflict) => (
                <p
                  key={
                    conflict.kind === 'duplicate'
                      ? `duplicate:${conflict.otherId}`
                      : `reserved:${conflict.reservedId}`
                  }
                  className="break-words text-destructive"
                >
                  {conflict.kind === 'duplicate'
                    ? t('inspector.conflictDuplicate', { id: conflict.otherId })
                    : t('inspector.conflictReserved', { id: conflict.reservedId })}
                </p>
              ))}
            </li>
          )
        })}
      </ContributionSection>
      <ContributionSection
        title={t('inspector.statusItems')}
        empty={t('inspector.emptyStatusItems')}
        count={statusItems.length}
      >
        {statusItems.map((item) => (
          <li key={item.id} className={rowClass}>
            <p className="break-words font-medium">{item.label}</p>
            <div className={detailsClass}>
              <Detail label={t('inspector.id')}>{item.id}</Detail>
              <Detail label={t('inspector.location')}>{item.location}</Detail>
              <Detail label={t('inspector.scope')}>{item.scope}</Detail>
              {item.commandId && <Detail label={t('inspector.commandId')}>{item.commandId}</Detail>}
            </div>
          </li>
        ))}
      </ContributionSection>
      <ContributionSection
        title={t('inspector.eventSubscriptions')}
        empty={t('inspector.emptyEventSubscriptions')}
        count={eventSubscriptions.length}
      >
        {eventSubscriptions.map((subscription) => (
          <li key={subscription.id} className={`${rowClass} ${detailsClass}`}>
            <Detail label={t('inspector.id')}>{subscription.id}</Detail>
            <Detail label={t('inspector.topic')}>{subscription.topic}</Detail>
            <Detail label={t('inspector.scope')}>{subscription.scope}</Detail>
          </li>
        ))}
      </ContributionSection>
    </div>
  )
}
