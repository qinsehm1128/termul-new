import type {
  ExecuteContributionRequest,
  HostContextState,
  PluginMenu
} from '@shared/types/plugin-host.types'
import { Puzzle } from 'lucide-react'
import {
  ContextMenuGroup,
  ContextMenuItem,
  ContextMenuSeparator
} from '@/components/ui/context-menu'
import { usePluginsStore } from '@/stores/plugins-store'
import { type ContributionContext, matchesHostWhen } from './contribution-context'
import { usePluginHostUiStore } from './host-ui-store'
import { pluginApi } from './plugin-api'

export interface PluginMenuItemsProps {
  location: PluginMenu['location']
  /** IDs belong to the right-clicked object, never the currently active tab/project. */
  target: ExecuteContributionRequest['target']
  terminalFocused?: boolean
  textInputFocused?: boolean
}

function targetAvailable(
  location: PluginMenu['location'],
  target: ExecuteContributionRequest['target'],
  context: HostContextState | null,
  revision: number
): boolean {
  if (!context || revision <= 0) return false
  if (location.startsWith('terminal.') && !target.terminalId) return false
  if (location === 'project.context' && !target.projectId) return false
  if (location === 'agentSession.context' && !target.sessionId) return false
  if (target.projectId && !context.projectIds.includes(target.projectId)) return false
  if (
    target.terminalId &&
    !context.terminals.some(
      (terminal) =>
        terminal.id === target.terminalId &&
        (!target.projectId || terminal.projectId === target.projectId)
    )
  )
    return false
  if (
    target.sessionId &&
    !context.agentSessions.some(
      (session) =>
        session.id === target.sessionId &&
        (!target.projectId || session.projectId === target.projectId)
    )
  )
    return false
  return true
}

/** Appended once, after native actions; plugin labels never impersonate native actions. */
export function PluginMenuItems({
  location,
  target,
  terminalFocused,
  textInputFocused
}: PluginMenuItemsProps): React.JSX.Element | null {
  const plugins = usePluginsStore((state) => state.plugins)
  const connected = usePluginsStore((state) => state.connected)
  const acknowledgedContext = usePluginHostUiStore((state) => state.context)
  const contextRevision = usePluginHostUiStore((state) => state.contextRevision)
  if (!connected || !targetAvailable(location, target, acknowledgedContext, contextRevision))
    return null

  const context: ContributionContext = {
    'project.exists': Boolean(target.projectId),
    'terminal.exists': Boolean(target.terminalId),
    'agentSession.exists': Boolean(target.sessionId),
    'terminal.focused': terminalFocused ?? acknowledgedContext?.focus.terminal ?? false,
    'textInput.focused': textInputFocused ?? acknowledgedContext?.focus.textInput ?? false
  }
  const items = plugins
    .filter((plugin) => plugin.state === 'active' && plugin.desiredEnabled && plugin.generation)
    .flatMap((plugin) =>
      (plugin.manifest.contributes.menus ?? [])
        .filter((menu) => menu.location === location && matchesHostWhen(menu.when, context))
        .flatMap((menu) => {
          const command = plugin.manifest.contributes.commands.find(
            (entry) => entry.id === menu.commandId
          )
          return command ? [{ plugin, menu, command }] : []
        })
    )
    .sort(
      (a, b) =>
        (a.menu.order ?? 0) - (b.menu.order ?? 0) ||
        (a.plugin.id < b.plugin.id ? -1 : a.plugin.id > b.plugin.id ? 1 : 0) ||
        (a.menu.id < b.menu.id ? -1 : a.menu.id > b.menu.id ? 1 : 0)
    )
  if (!items.length) return null

  return (
    <>
      <ContextMenuSeparator />
      <ContextMenuGroup aria-label="Plugins">
        {items.map(({ plugin, menu, command }) => (
          <ContextMenuItem
            key={`${plugin.id}:${menu.id}`}
            onSelect={() => {
              // Revalidate the lease at click time as well as render time.
              const live = usePluginsStore.getState()
              const host = usePluginHostUiStore.getState()
              const current = live.plugins.find((entry) => entry.id === plugin.id)
              if (
                !live.connected ||
                !targetAvailable(location, target, host.context, host.contextRevision) ||
                !matchesHostWhen(menu.when, {
                  'project.exists': Boolean(target.projectId),
                  'terminal.exists': Boolean(target.terminalId),
                  'agentSession.exists': Boolean(target.sessionId),
                  'terminal.focused': terminalFocused ?? host.context?.focus.terminal ?? false,
                  'textInput.focused': textInputFocused ?? host.context?.focus.textInput ?? false
                }) ||
                current?.state !== 'active' ||
                !current.desiredEnabled ||
                !current.generation ||
                current.generation !== plugin.generation
              )
                return
              void pluginApi
                .executeContribution({
                  pluginId: plugin.id,
                  generation: current.generation,
                  kind: 'menu',
                  contributionId: menu.id,
                  contextRevision: host.contextRevision,
                  target
                })
                .catch(() => undefined)
            }}
          >
            <Puzzle className="mr-2 h-4 w-4 shrink-0" aria-hidden="true" />
            <span>{command.title}</span>
            <span className="ml-2 text-xs text-muted-foreground">{plugin.manifest.name}</span>
          </ContextMenuItem>
        ))}
      </ContextMenuGroup>
    </>
  )
}
