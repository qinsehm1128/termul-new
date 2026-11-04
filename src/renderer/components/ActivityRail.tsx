import {
  isUserDividerId,
  RAIL_PLUGIN_VIEWS_DIVIDER_ID,
  RAIL_UTILITY_ITEM_IDS,
  type RailDividerId,
  type RailItemId,
  type RailLayoutEntry,
  type RailReorderFeedback,
  railItemMobility
} from '@shared/types/navigation.types'
import {
  BrainCircuit,
  CalendarClock,
  ChevronDown,
  ChevronRight,
  FolderKanban,
  GitBranch,
  History,
  MessageSquarePlus,
  Network,
  Palette,
  Plug,
  Puzzle,
  SlidersHorizontal,
  Sparkles,
  SquareTerminal,
  Terminal as TerminalIcon
} from 'lucide-react'
import {
  cloneElement,
  type KeyboardEvent,
  type MouseEvent,
  type ReactElement,
  type PointerEvent as ReactPointerEvent,
  useEffect,
  useRef,
  useState
} from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
import { SeMark } from '@/components/SeMark'
import { TitleBarShortcutsPopover } from '@/components/TitleBarShortcutsPopover'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger
} from '@/components/ui/context-menu'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { getPluginViewIcon } from '@/features/plugins/view-icons'
import { useUpdatePanelVisibility } from '@/hooks/use-app-settings'
import { useNavigationLayout } from '@/hooks/use-navigation-layout'
import { isMac } from '@/lib/platform'
import { type RailRowBox, railDragExceeded, railPointerDrop, railRestIndex } from '@/lib/rail-drop'
import { isConversationAreaPath } from '@/lib/router-navigate'
import { isTauriContext } from '@/lib/tauri-runtime'
import { isTerminalRunning } from '@/lib/terminal-board'
import { cn } from '@/lib/utils'
import { useNavigationStore } from '@/stores/navigation-store'
import { usePluginsStore } from '@/stores/plugins-store'
import { useSSHPanelVisible } from '@/stores/ssh-panel-store'
import { useSSHStore } from '@/stores/ssh-store'
import { useTerminalStore } from '@/stores/terminal-store'

const REORDER_HINT_ID = 'activity-rail-reorder-hint'

const railButtonClass =
  'relative mx-1 flex h-9 w-9 items-center justify-center rounded-md text-muted-foreground transition-[color,background-color,box-shadow] duration-150 ease-[var(--ease-out)] before:absolute before:-left-1 before:h-4 before:w-px before:rounded-full before:bg-primary before:opacity-0 before:transition-opacity hover:bg-foreground/[0.045] hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring/80 aria-[pressed=true]:bg-foreground/[0.065] aria-[pressed=true]:text-foreground aria-[pressed=true]:shadow-[inset_0_1px_0_hsl(var(--foreground)/0.035)] aria-[pressed=true]:before:opacity-100 aria-[current=page]:bg-foreground/[0.065] aria-[current=page]:text-foreground aria-[current=page]:before:opacity-100 disabled:opacity-30'

const ITEM_LABEL_KEY: Record<RailItemId, string> = {
  projects: 'activityRail.projects',
  terminals: 'activityRail.terminals',
  gitChanges: 'activityRail.gitChanges',
  gitHistory: 'activityRail.gitHistory',
  ssh: 'activityRail.toggleSsh',
  conversations: 'activityRail.conversations',
  quickTerminals: 'activityRail.quickTerminals',
  skills: 'activityRail.skills',
  aiChannels: 'activityRail.aiChannels',
  mcp: 'activityRail.mcp',
  plugins: 'activityRail.plugins',
  scheduledTasks: 'activityRail.scheduledTasks',
  shortcuts: 'titleBar.keyboardShortcuts',
  preferences: 'activityRail.preferences',
  themes: 'activityRail.colorThemes'
}

function RailTooltip({
  label,
  disabled = false,
  children
}: {
  label: string
  disabled?: boolean
  children: ReactElement
}): React.JSX.Element {
  const control = cloneElement(children, {
    'aria-keyshortcuts': 'Alt+ArrowUp Alt+ArrowDown',
    'aria-describedby': REORDER_HINT_ID
  })
  return (
    <Tooltip>
      <TooltipTrigger asChild data-rail-tooltip={label}>
        {disabled ? <span className="block leading-none">{control}</span> : control}
      </TooltipTrigger>
      <TooltipContent side="right" sideOffset={9}>
        {label}
      </TooltipContent>
    </Tooltip>
  )
}

interface ActivityRailProps {
  isShortcutsOpen?: boolean
  onShortcutsOpenChange?: (open: boolean) => void
  /** Opens a git changes tab in the active pane. */
  onOpenGitChanges?: () => void
  /** Whether a git changes tab can currently be opened (active project has a path). */
  canOpenGitChanges?: boolean
  /** Opens a git history (commit graph) tab in the active pane. */
  onOpenGitHistory?: () => void
  /** Whether a git history tab can currently be opened (active project has a path). */
  canOpenGitHistory?: boolean
  /** Whether the color theme picker overlay is open. */
  isThemePickerOpen?: boolean
  /** Toggle the color theme picker (opens beside the rail). */
  onToggleThemePicker?: () => void
}

interface RailDropState {
  movingId: RailItemId
  beforeEntryIndex: number
  pin: boolean
}

interface RailSectionModel {
  key: string
  sectionId: 'projects' | 'conversations' | 'tools'
  label: string
  dividerBefore: { id: RailDividerId; entryIndex: number; collapsed: boolean } | null
  items: Array<{ id: RailItemId; entryIndex: number }>
}

function readRailRows(root: HTMLElement): RailRowBox[] {
  return [...root.querySelectorAll<HTMLElement>('[data-rail-row]')].map((element) => {
    const rect = element.getBoundingClientRect()
    const kind = element.dataset.railKind
    return {
      kind: kind === 'divider' || kind === 'utility' ? kind : 'item',
      id: element.dataset.railEntry ?? '',
      entryIndex: Number(element.dataset.railIndex ?? '-1'),
      top: rect.top,
      height: rect.height
    }
  })
}

function neighborLabel(
  feedback: Extract<RailReorderFeedback, { kind: 'moved-before' | 'moved-after' }>,
  translate: (key: string) => string
): string {
  if (feedback.neighborKind === 'divider') {
    if (feedback.neighborId === RAIL_PLUGIN_VIEWS_DIVIDER_ID)
      return translate('activityRail.plugins')
    if (feedback.neighborId === 'workspace-contexts') {
      return translate('activityRail.dividerWorkspace')
    }
    return isUserDividerId(feedback.neighborId)
      ? translate('activityRail.dividerCustom')
      : translate('activityRail.dividerTools')
  }
  return translate(ITEM_LABEL_KEY[feedback.neighborId as RailItemId])
}

function formatRailFeedback(
  feedback: RailReorderFeedback,
  translate: (key: string, options?: Record<string, string>) => string
): string {
  const item = translate(ITEM_LABEL_KEY[feedback.id])
  switch (feedback.kind) {
    case 'moved-before':
      return translate('activityRail.movedBefore', {
        item,
        neighbor: neighborLabel(feedback, translate)
      })
    case 'moved-after':
      return translate('activityRail.movedAfter', {
        item,
        neighbor: neighborLabel(feedback, translate)
      })
    case 'pinned':
      return translate('activityRail.pinnedBottom', { item })
    case 'unchanged':
      if (feedback.reason === 'first') return translate('activityRail.alreadyFirst', { item })
      if (feedback.reason === 'last') return translate('activityRail.alreadyLast', { item })
      return translate('activityRail.reorderUnchanged', { item })
  }
}

/**
 * Vertical activity rail (VSCode-style) that hosts the app's global actions.
 *
 * Layout:
 * - macOS: WorkspaceLayout renders a full-width titlebar zone above this rail;
 *   the brand row stays draggable for top-left window moves and is not an icon.
 * - Brand mark at the top, followed by a separator. The mark is fixed.
 * - Sortable icons and structural dividers follow the global rail layout.
 *   Dividers can be crossed but are not drag sources. Routes, pressed/current
 *   state, tooltips, and the command palette are keyed by item id, not by
 *   visual position.
 * - Utility actions (shortcuts, preferences, themes) stay in the bottom group
 *   unless a saved layout opts them into the sortable region.
 */
export function ActivityRail({
  isShortcutsOpen,
  onShortcutsOpenChange,
  onOpenGitChanges,
  canOpenGitChanges = false,
  onOpenGitHistory,
  canOpenGitHistory = false,
  isThemePickerOpen = false,
  onToggleThemePicker
}: ActivityRailProps = {}): React.JSX.Element {
  const { t } = useTranslation('shell')
  useNavigationLayout()
  const entries = useNavigationStore((state) => state.layout.entries)
  const plugins = usePluginsStore((state) => state.plugins)
  const pluginsConnected = usePluginsStore((state) => state.connected)
  const pluginViews = pluginsConnected
    ? plugins
        .filter((plugin) => plugin.state === 'active' && plugin.desiredEnabled)
        .sort((a, b) => a.manifest.name.localeCompare(b.manifest.name))
        .flatMap((plugin) =>
          plugin.manifest.contributes.views
            .filter((view) => view.location === 'sidebar')
            .map((view) => ({ plugin, view }))
        )
    : []
  const moveItemBy = useNavigationStore((state) => state.moveItemBy)
  const moveItemToIndex = useNavigationStore((state) => state.moveItemToIndex)
  const pinUtility = useNavigationStore((state) => state.pinUtility)
  const insertDivider = useNavigationStore((state) => state.insertDivider)
  const removeDivider = useNavigationStore((state) => state.removeDivider)
  const toggleDivider = useNavigationStore((state) => state.toggleDivider)
  const isSSHPanelVisible = useSSHPanelVisible()
  const updatePanelVisibility = useUpdatePanelVisibility()
  const navigate = useNavigate()
  const location = useLocation()
  // The shared predicate, not a copy of its body. The file-tree scope rule
  // drifted from exactly this kind of local re-derivation.
  const isConversationsActive = isConversationAreaPath(location.pathname)
  const isTerminalsActive = location.pathname === '/terminals'
  const liveTerminalCount = useTerminalStore(
    (state) => state.terminals.filter(isTerminalRunning).length
  )
  const [drop, setDrop] = useState<RailDropState | null>(null)
  const [reorderStatus, setReorderStatus] = useState('')
  const navRef = useRef<HTMLElement>(null)
  const suppressClickRef = useRef(false)
  const pendingFocusId = useRef<RailItemId | null>(null)
  const stopDragListeners = useRef<(() => void) | null>(null)
  const entryKey = entries.map((entry) => `${entry.kind}:${entry.id}`).join('|')

  useEffect(() => () => stopDragListeners.current?.(), [])

  // entryKey is a render-completion signal: the focus target is created by the entries render.
  // biome-ignore lint/correctness/useExhaustiveDependencies: rerun after the entry DOM commits
  useEffect(() => {
    const id = pendingFocusId.current
    if (!id || !navRef.current) return
    pendingFocusId.current = null
    const row = navRef.current.querySelector<HTMLElement>(`[data-rail-entry="${id}"]`)
    if (!row) return
    const button = row.querySelector<HTMLButtonElement>('button')
    const target = button && !button.disabled ? button : row
    target.focus()
  }, [entryKey])

  const handleToggleSSHPanel = async (event: MouseEvent<HTMLButtonElement>): Promise<void> => {
    event.stopPropagation()
    try {
      await updatePanelVisibility('sshPanelVisible', !isSSHPanelVisible)
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('activityRail.failedSsh'))
    }
  }

  const translate = (key: string, options?: Record<string, string>): string =>
    String(t(key as never, options as never))

  const announce = (feedback: RailReorderFeedback): void => {
    setReorderStatus(formatRailFeedback(feedback, translate))
  }

  const onReorderKey = (event: KeyboardEvent, id: RailItemId): void => {
    if (!event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return
    if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return
    event.preventDefault()
    event.stopPropagation()
    pendingFocusId.current = id
    announce(moveItemBy(id, event.key === 'ArrowUp' ? -1 : 1))
  }

  const startDrag = (event: ReactPointerEvent<HTMLElement>, id: RailItemId): void => {
    if (event.button !== 0 || event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) {
      return
    }
    stopDragListeners.current?.()
    const pointerId = event.pointerId
    const startY = event.clientY
    let dragging = false
    let dropTarget: { beforeEntryIndex: number; pin: boolean } | null = null

    const move = (pointerEvent: PointerEvent): void => {
      if (pointerEvent.pointerId !== pointerId) return
      if (!dragging) {
        if (!railDragExceeded(startY, pointerEvent.clientY)) return
        dragging = true
      }
      pointerEvent.preventDefault()
      const root = navRef.current
      if (!root) return
      dropTarget = railPointerDrop(pointerEvent.clientY, readRailRows(root), id)
      setDrop({ movingId: id, ...dropTarget })
    }

    const finish = (pointerEvent: PointerEvent): void => {
      if (pointerEvent.pointerId !== pointerId) return
      stop()
      const target = dropTarget
      setDrop(null)
      if (!dragging || !target) return
      suppressClickRef.current = true
      window.setTimeout(() => {
        suppressClickRef.current = false
      }, 0)
      if (target.pin) {
        announce(pinUtility(id))
        return
      }
      const index = railRestIndex(
        useNavigationStore.getState().layout.entries,
        id,
        target.beforeEntryIndex
      )
      announce(moveItemToIndex(id, index))
    }

    const stop = (): void => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', finish)
      window.removeEventListener('pointercancel', finish)
      stopDragListeners.current = null
    }

    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', finish)
    window.addEventListener('pointercancel', finish)
    stopDragListeners.current = stop
  }

  const renderButton = (id: RailItemId): { disabled: boolean; element: ReactElement } => {
    const activate = (action: () => void) => (event: MouseEvent<HTMLButtonElement>) => {
      if (suppressClickRef.current) {
        suppressClickRef.current = false
        event.preventDefault()
        event.stopPropagation()
        return
      }
      event.stopPropagation()
      // A selected SSH profile takes over the main area on every route, so a
      // rail pick that leaves it selected changes the route behind the SSH
      // view and nothing visible happens. The connection itself stays up.
      useSSHStore.getState().selectProfile(null)
      action()
    }

    switch (id) {
      case 'projects':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.projects')}>
              <button
                type="button"
                onClick={activate(() => navigate('/'))}
                className={railButtonClass}
                aria-label={t('activityRail.openProjects')}
                aria-pressed={location.pathname === '/'}
              >
                <FolderKanban
                  size={18}
                  className={cn(
                    'transition-colors',
                    location.pathname === '/' ? 'text-foreground' : 'text-muted-foreground'
                  )}
                />
              </button>
            </RailTooltip>
          )
        }
      case 'terminals':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.terminals')}>
              <button
                type="button"
                onClick={activate(() => navigate(isTerminalsActive ? '/' : '/terminals'))}
                className={railButtonClass}
                aria-label={t('activityRail.openTerminals')}
                aria-current={isTerminalsActive ? 'page' : undefined}
              >
                <SquareTerminal
                  size={18}
                  className={cn(
                    'transition-colors',
                    isTerminalsActive ? 'text-foreground' : 'text-muted-foreground'
                  )}
                />
                {liveTerminalCount > 0 ? (
                  <span
                    className="absolute right-0.5 top-0.5 min-w-3 rounded-sm bg-primary px-0.5 text-center text-[9px] leading-3 text-primary-foreground"
                    aria-hidden="true"
                  >
                    {liveTerminalCount > 99 ? '99+' : liveTerminalCount}
                  </span>
                ) : null}
              </button>
            </RailTooltip>
          )
        }
      case 'gitChanges': {
        const disabled = !onOpenGitChanges || !canOpenGitChanges
        return {
          disabled,
          element: (
            <RailTooltip
              disabled={disabled}
              label={
                canOpenGitChanges
                  ? t('activityRail.gitChanges')
                  : t('activityRail.gitChangesNeedsProject')
              }
            >
              <button
                type="button"
                onClick={activate(() => onOpenGitChanges?.())}
                className={cn(railButtonClass, disabled && 'pointer-events-none')}
                aria-label={t('activityRail.openGitChanges')}
                disabled={disabled}
              >
                <GitBranch
                  size={18}
                  className={
                    canOpenGitChanges ? 'text-muted-foreground' : 'text-muted-foreground/40'
                  }
                />
              </button>
            </RailTooltip>
          )
        }
      }
      case 'gitHistory': {
        const disabled = !onOpenGitHistory || !canOpenGitHistory
        return {
          disabled,
          element: (
            <RailTooltip
              disabled={disabled}
              label={
                canOpenGitHistory
                  ? t('activityRail.gitHistory')
                  : t('activityRail.gitHistoryNeedsProject')
              }
            >
              <button
                type="button"
                onClick={activate(() => onOpenGitHistory?.())}
                className={cn(railButtonClass, disabled && 'pointer-events-none')}
                aria-label={t('activityRail.openGitHistory')}
                disabled={disabled}
              >
                <History
                  size={18}
                  className={
                    canOpenGitHistory ? 'text-muted-foreground' : 'text-muted-foreground/40'
                  }
                />
              </button>
            </RailTooltip>
          )
        }
      }
      case 'ssh': {
        const disabled = !isTauriContext()
        return {
          disabled,
          element: (
            <RailTooltip
              disabled={disabled}
              label={
                isTauriContext() ? t('activityRail.toggleSsh') : t('activityRail.sshDesktopOnly')
              }
            >
              <button
                type="button"
                onClick={(event) => {
                  if (suppressClickRef.current) {
                    suppressClickRef.current = false
                    event.preventDefault()
                    event.stopPropagation()
                    return
                  }
                  void handleToggleSSHPanel(event)
                }}
                className={cn(railButtonClass, disabled && 'pointer-events-none')}
                aria-label={
                  isSSHPanelVisible ? t('activityRail.hideSsh') : t('activityRail.showSsh')
                }
                aria-pressed={isSSHPanelVisible}
                disabled={disabled}
              >
                <Network
                  size={18}
                  className={
                    isSSHPanelVisible
                      ? 'text-foreground'
                      : isTauriContext()
                        ? 'text-muted-foreground'
                        : 'text-muted-foreground/40'
                  }
                />
              </button>
            </RailTooltip>
          )
        }
      }
      case 'conversations':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.conversations')}>
              <button
                type="button"
                onClick={activate(() => navigate(isConversationsActive ? '/' : '/conversations'))}
                className={railButtonClass}
                aria-label={t('activityRail.openConversations')}
                aria-pressed={isConversationsActive}
              >
                <MessageSquarePlus
                  size={18}
                  className={cn(
                    'transition-colors',
                    isConversationsActive ? 'text-foreground' : 'text-muted-foreground'
                  )}
                />
              </button>
            </RailTooltip>
          )
        }
      case 'skills':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.skills')}>
              <button
                type="button"
                onClick={activate(() => navigate('/skills'))}
                className={railButtonClass}
                aria-label={t('activityRail.openSkills')}
                aria-current={location.pathname === '/skills' ? 'page' : undefined}
              >
                <Sparkles
                  size={18}
                  className={
                    location.pathname === '/skills' ? 'text-foreground' : 'text-muted-foreground'
                  }
                />
              </button>
            </RailTooltip>
          )
        }
      case 'aiChannels':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.aiChannels')}>
              <button
                type="button"
                onClick={activate(() => navigate('/ai-channels'))}
                className={railButtonClass}
                aria-label={t('activityRail.openAiChannels')}
                aria-current={location.pathname === '/ai-channels' ? 'page' : undefined}
              >
                <BrainCircuit
                  size={18}
                  className={
                    location.pathname === '/ai-channels'
                      ? 'text-foreground'
                      : 'text-muted-foreground'
                  }
                />
              </button>
            </RailTooltip>
          )
        }
      case 'mcp':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.mcp')}>
              <button
                type="button"
                onClick={activate(() => navigate('/mcp'))}
                className={railButtonClass}
                aria-label={t('activityRail.openMcp')}
                aria-current={location.pathname === '/mcp' ? 'page' : undefined}
              >
                <Plug
                  size={18}
                  className={
                    location.pathname === '/mcp' ? 'text-foreground' : 'text-muted-foreground'
                  }
                />
              </button>
            </RailTooltip>
          )
        }
      case 'plugins':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.plugins')}>
              <button
                type="button"
                onClick={activate(() => navigate('/plugins'))}
                className={railButtonClass}
                aria-label={t('activityRail.openPlugins')}
                aria-current={location.pathname === '/plugins' ? 'page' : undefined}
              >
                <Puzzle
                  size={18}
                  className={
                    location.pathname === '/plugins' ? 'text-foreground' : 'text-muted-foreground'
                  }
                />
              </button>
            </RailTooltip>
          )
        }
      case 'quickTerminals': {
        const active = location.pathname.startsWith('/quick-terminals')
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.quickTerminals')}>
              <button
                type="button"
                onClick={activate(() => navigate('/quick-terminals'))}
                className={railButtonClass}
                aria-label={t('activityRail.openQuickTerminals')}
                aria-current={active ? 'page' : undefined}
              >
                <TerminalIcon
                  size={18}
                  className={cn(
                    'transition-colors',
                    active ? 'text-foreground' : 'text-muted-foreground'
                  )}
                />
              </button>
            </RailTooltip>
          )
        }
      }
      case 'scheduledTasks':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.scheduledTasks')}>
              <button
                type="button"
                onClick={activate(() => navigate('/scheduled-tasks'))}
                className={railButtonClass}
                aria-label={t('activityRail.openScheduledTasks')}
                aria-current={location.pathname === '/scheduled-tasks' ? 'page' : undefined}
              >
                <CalendarClock
                  size={18}
                  className={cn(
                    'transition-colors',
                    location.pathname === '/scheduled-tasks'
                      ? 'text-foreground'
                      : 'text-muted-foreground'
                  )}
                />
              </button>
            </RailTooltip>
          )
        }
      case 'shortcuts':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('titleBar.keyboardShortcuts')}>
              <TitleBarShortcutsPopover
                buttonClassName={railButtonClass}
                open={isShortcutsOpen}
                onOpenChange={onShortcutsOpenChange}
              />
            </RailTooltip>
          )
        }
      case 'preferences':
        return {
          disabled: false,
          element: (
            <RailTooltip label={t('activityRail.preferences')}>
              <button
                type="button"
                onClick={activate(() => navigate('/preferences'))}
                className={railButtonClass}
                aria-label={t('activityRail.openPreferences')}
                aria-current={location.pathname === '/preferences' ? 'page' : undefined}
              >
                <SlidersHorizontal
                  size={18}
                  className={
                    location.pathname === '/preferences'
                      ? 'text-foreground'
                      : 'text-muted-foreground'
                  }
                />
              </button>
            </RailTooltip>
          )
        }
      case 'themes': {
        const disabled = !onToggleThemePicker
        return {
          disabled,
          element: (
            <RailTooltip disabled={disabled} label={t('activityRail.colorThemes')}>
              <button
                type="button"
                onClick={
                  onToggleThemePicker
                    ? activate(() => onToggleThemePicker())
                    : (event) => {
                        if (suppressClickRef.current) {
                          suppressClickRef.current = false
                          event.preventDefault()
                          event.stopPropagation()
                        }
                      }
                }
                className={cn(railButtonClass, disabled && 'pointer-events-none')}
                aria-label={t('activityRail.colorThemes')}
                aria-pressed={onToggleThemePicker ? isThemePickerOpen : undefined}
                aria-disabled={!onToggleThemePicker}
                disabled={disabled}
              >
                <Palette
                  size={18}
                  className={isThemePickerOpen ? 'text-foreground' : 'text-muted-foreground'}
                />
              </button>
            </RailTooltip>
          )
        }
      }
    }
  }

  // Radix picks the innermost trigger; stopping propagation keeps the global
  // copy/paste menu from also handling a rail right-click.
  const withRailMenu = (key: string, row: ReactElement, items: ReactElement): ReactElement => (
    <ContextMenu key={key}>
      <ContextMenuTrigger asChild onContextMenu={(event) => event.stopPropagation()}>
        {row}
      </ContextMenuTrigger>
      <ContextMenuContent>{items}</ContextMenuContent>
    </ContextMenu>
  )

  const renderSortable = (
    id: RailItemId,
    entryIndex: number | null,
    showAfterIndicator = false
  ): ReactElement => {
    const button = renderButton(id)
    const showIndicator =
      entryIndex !== null && drop !== null && !drop.pin && drop.beforeEntryIndex === entryIndex
    const row = (
      <div
        key={id}
        data-rail-entry={id}
        data-rail-kind="item"
        data-rail-mobility={railItemMobility(id)}
        data-rail-index={entryIndex === null ? undefined : String(entryIndex)}
        data-rail-row={entryIndex === null ? undefined : 'true'}
        data-rail-dragging={drop?.movingId === id ? 'true' : undefined}
        className={cn('relative flex justify-center', drop?.movingId === id && 'opacity-60')}
        tabIndex={button.disabled ? 0 : undefined}
        aria-label={button.disabled ? translate(ITEM_LABEL_KEY[id]) : undefined}
        aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
        aria-describedby={REORDER_HINT_ID}
        onPointerDown={(event) => startDrag(event, id)}
        onKeyDown={(event) => onReorderKey(event, id)}
      >
        {showIndicator ? (
          <div
            data-rail-drop-indicator="before"
            data-rail-drop-before={id}
            className="pointer-events-none absolute left-1/2 top-0 z-10 h-0.5 w-6 -translate-x-1/2 rounded-full bg-primary"
            aria-hidden="true"
          />
        ) : null}
        {showAfterIndicator ? (
          <div
            data-rail-drop-indicator="after"
            className="pointer-events-none absolute bottom-0 left-1/2 z-10 h-0.5 w-8 -translate-x-1/2 rounded-full bg-primary"
            aria-hidden="true"
          />
        ) : null}
        {button.element}
      </div>
    )
    // Pinned utilities have no place in the entry list to insert beside.
    if (entryIndex === null) return row
    return withRailMenu(
      id,
      row,
      <>
        <ContextMenuItem onSelect={() => insertDivider(entryIndex)}>
          {t('activityRail.insertDividerAbove')}
        </ContextMenuItem>
        <ContextMenuItem onSelect={() => insertDivider(entryIndex + 1)}>
          {t('activityRail.insertDividerBelow')}
        </ContextMenuItem>
      </>
    )
  }

  const dividerLabel = (id: RailDividerId): string => {
    if (id === 'workspace-contexts') return t('activityRail.dividerWorkspace')
    if (id === RAIL_PLUGIN_VIEWS_DIVIDER_ID) return t('activityRail.plugins')
    return isUserDividerId(id) ? t('activityRail.dividerCustom') : t('activityRail.dividerTools')
  }

  const renderDivider = (
    id: RailDividerId,
    entryIndex: number,
    collapsed: boolean,
    hiddenCount: number
  ): ReactElement => {
    const label = dividerLabel(id)
    const tooltip = collapsed
      ? t('activityRail.collapsedGroup', { group: label, count: hiddenCount })
      : label
    const Chevron = collapsed ? ChevronRight : ChevronDown
    const row = (
      <div key={id} className="relative flex w-full justify-center">
        {drop && !drop.pin && drop.beforeEntryIndex === entryIndex ? (
          <div
            data-rail-drop-indicator="before"
            data-rail-drop-before={id}
            className="pointer-events-none absolute left-1/2 top-0 z-10 h-0.5 w-6 -translate-x-1/2 rounded-full bg-primary"
            aria-hidden="true"
          />
        ) : null}
        {entryIndex >= 0 &&
        collapsed &&
        drop &&
        !drop.pin &&
        drop.beforeEntryIndex === entryIndex + 1 ? (
          <div
            data-rail-drop-indicator="after"
            className="pointer-events-none absolute bottom-0 left-1/2 z-10 h-0.5 w-6 -translate-x-1/2 rounded-full bg-primary"
            aria-hidden="true"
          />
        ) : null}
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              className="flex h-6 w-full items-center justify-center gap-0.5 rounded-sm text-muted-foreground/70 hover:bg-foreground/[0.045] hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              data-activity-rail-divider={id}
              data-rail-row={entryIndex >= 0 ? 'true' : undefined}
              data-rail-entry={id}
              data-rail-kind="divider"
              data-rail-index={entryIndex >= 0 ? entryIndex : undefined}
              aria-label={label}
              aria-expanded={!collapsed}
              onClick={() => toggleDivider(id)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault()
                  if (!event.repeat) toggleDivider(id)
                }
              }}
            >
              <Chevron size={12} aria-hidden="true" />
              {collapsed ? (
                <span className="text-[9px] leading-none" aria-hidden="true">
                  {hiddenCount}
                </span>
              ) : (
                <span className="h-px w-3 bg-sidebar-border/80" aria-hidden="true" />
              )}
            </button>
          </TooltipTrigger>
          <TooltipContent side="right" sideOffset={9}>
            {tooltip}
          </TooltipContent>
        </Tooltip>
      </div>
    )
    return withRailMenu(
      id,
      row,
      <>
        <ContextMenuItem onSelect={() => toggleDivider(id)}>
          {collapsed ? t('activityRail.expandGroup') : t('activityRail.collapseGroup')}
        </ContextMenuItem>
        {isUserDividerId(id) ? (
          <ContextMenuItem onSelect={() => removeDivider(id)}>
            {t('activityRail.removeDivider')}
          </ContextMenuItem>
        ) : null}
      </>
    )
  }

  // Collapsing never conceals the current route or an open rail-controlled panel.
  const isItemActive = (id: RailItemId): boolean => {
    switch (id) {
      case 'projects':
        return location.pathname === '/'
      case 'conversations':
        return isConversationsActive
      case 'terminals':
        return isTerminalsActive
      case 'quickTerminals':
        return location.pathname.startsWith('/quick-terminals')
      case 'skills':
        return location.pathname === '/skills'
      case 'aiChannels':
        return location.pathname === '/ai-channels'
      case 'mcp':
        return location.pathname === '/mcp'
      case 'plugins':
        return location.pathname === '/plugins'
      case 'scheduledTasks':
        return location.pathname === '/scheduled-tasks'
      case 'preferences':
        return location.pathname === '/preferences'
      case 'ssh':
        return isSSHPanelVisible
      case 'shortcuts':
        return Boolean(isShortcutsOpen)
      case 'themes':
        return isThemePickerOpen
      default:
        return false
    }
  }
  const sections = buildSections(
    entries,
    t('activityRail.projectActions'),
    t('activityRail.conversationActions'),
    t('activityRail.toolsActions')
  )
  const pluginDivider = entries.find(
    (entry) => entry.kind === 'divider' && entry.id === RAIL_PLUGIN_VIEWS_DIVIDER_ID
  )
  const pluginsCollapsed = pluginDivider?.kind === 'divider' && Boolean(pluginDivider.collapsed)
  const pluginDividerIndex = entries.findIndex((entry) => entry.id === RAIL_PLUGIN_VIEWS_DIVIDER_ID)
  const sortableEndIndex = pluginDividerIndex < 0 ? entries.length : pluginDividerIndex
  const pluginPath = (pluginId: string, viewId: string): string =>
    `/plugins/${encodeURIComponent(pluginId)}/views/${encodeURIComponent(viewId)}`
  const visiblePluginViews = pluginViews.filter(
    ({ plugin, view }) => !pluginsCollapsed || location.pathname === pluginPath(plugin.id, view.id)
  )
  const pinnedUtilities = RAIL_UTILITY_ITEM_IDS.filter(
    (id) => !entries.some((entry) => entry.kind === 'item' && entry.id === id)
  )

  return (
    <TooltipProvider delayDuration={120} skipDelayDuration={200}>
      <nav
        ref={navRef}
        className="flex w-11 shrink-0 select-none flex-col items-center border-r border-sidebar-border/70 bg-sidebar shadow-[inset_-1px_0_0_hsl(var(--background)/0.35)]"
        aria-label={t('activityRail.globalActions')}
      >
        <p id={REORDER_HINT_ID} className="sr-only">
          {translate('activityRail.reorderHint')}
        </p>
        <div className="sr-only" aria-live="polite" data-testid="activity-rail-reorder-status">
          {reorderStatus}
        </div>

        {/* Brand mark. Fixed: not a layout entry and not a reorder target. */}
        <div
          className="flex h-9 w-11 shrink-0 items-center justify-center text-foreground/90"
          data-tauri-drag-region={isMac ? true : undefined}
          data-rail-brand="true"
        >
          <SeMark size={19} className="pointer-events-none" />
        </div>

        <div className="my-1 h-px w-4 bg-border/70" aria-hidden="true" />

        {sections.map((section) => {
          const collapsed = Boolean(section.dividerBefore?.collapsed)
          const visibleItems = section.items.filter((item) => !collapsed || isItemActive(item.id))
          return (
            <div key={section.key} className="flex w-full flex-col items-center">
              {section.dividerBefore
                ? renderDivider(
                    section.dividerBefore.id,
                    section.dividerBefore.entryIndex,
                    collapsed,
                    section.items.length - visibleItems.length
                  )
                : null}
              {visibleItems.length > 0 ? (
                <fieldset
                  aria-label={section.label}
                  data-activity-rail-section={section.sectionId}
                  className={cn(
                    'm-0 flex min-w-0 flex-col items-center border-0 p-0',
                    section.sectionId === 'projects' && 'gap-0.5'
                  )}
                >
                  {visibleItems.map((item) =>
                    renderSortable(
                      item.id,
                      item.entryIndex,
                      collapsed &&
                        drop !== null &&
                        !drop.pin &&
                        drop.beforeEntryIndex === item.entryIndex + 1 &&
                        section.items.some((next) => next.entryIndex === drop.beforeEntryIndex) &&
                        !visibleItems.some((next) => next.entryIndex === drop.beforeEntryIndex)
                    )
                  )}
                </fieldset>
              ) : null}
            </div>
          )
        })}

        {drop && !drop.pin && drop.beforeEntryIndex === sortableEndIndex ? (
          <div className="relative h-0 w-full">
            <div
              data-rail-drop-indicator="before"
              data-rail-drop-before="end"
              className="pointer-events-none absolute left-1/2 top-0 z-10 h-0.5 w-6 -translate-x-1/2 rounded-full bg-primary"
              aria-hidden="true"
            />
          </div>
        ) : null}

        {/* Plugin views are runtime contributions, never persisted or reorder targets. */}
        {pluginViews.length > 0 ? (
          <div className="flex w-full flex-col items-center">
            {renderDivider(
              RAIL_PLUGIN_VIEWS_DIVIDER_ID,
              -1,
              pluginsCollapsed,
              pluginViews.length - visiblePluginViews.length
            )}
            <fieldset
              aria-label={t('activityRail.pluginViews')}
              data-activity-rail-section="plugins"
              className="m-0 flex min-w-0 flex-col items-center border-0 p-0"
            >
              {visiblePluginViews.map(({ plugin, view }) => {
                const path = pluginPath(plugin.id, view.id)
                const active = location.pathname === path
                const Icon = getPluginViewIcon(view.icon)
                const label = `${view.title} (${plugin.manifest.name})`
                return (
                  <Tooltip key={`${plugin.id}/${view.id}`}>
                    <TooltipTrigger asChild data-rail-tooltip={label}>
                      <button
                        type="button"
                        className={railButtonClass}
                        aria-label={view.title}
                        aria-current={active ? 'page' : undefined}
                        onClick={(event) => {
                          event.stopPropagation()
                          useSSHStore.getState().selectProfile(null)
                          navigate(path)
                        }}
                      >
                        <Icon
                          size={18}
                          className={cn(
                            'transition-colors',
                            active ? 'text-foreground' : 'text-muted-foreground'
                          )}
                        />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent side="right" sideOffset={9}>
                      {label}
                    </TooltipContent>
                  </Tooltip>
                )
              })}
            </fieldset>
          </div>
        ) : null}

        <div
          className="relative mt-auto flex w-full flex-col items-center pb-1"
          data-rail-utility=""
          data-rail-row="true"
          data-rail-kind="utility"
          data-rail-entry="utility"
        >
          {drop?.pin ? (
            <div
              data-rail-drop-indicator="pin"
              className="pointer-events-none absolute left-1/2 top-0 z-10 h-0.5 w-6 -translate-x-1/2 rounded-full bg-primary"
              aria-hidden="true"
            />
          ) : null}
          {pinnedUtilities.map((id) => renderSortable(id, null))}
        </div>
      </nav>
    </TooltipProvider>
  )
}

function buildSections(
  entries: readonly RailLayoutEntry[],
  projectLabel: string,
  conversationLabel: string,
  toolsLabel: string
): RailSectionModel[] {
  const sections: RailSectionModel[] = []
  let section: RailSectionModel = {
    key: 'projects',
    sectionId: 'projects',
    label: projectLabel,
    dividerBefore: null,
    items: []
  }
  entries.forEach((entry, entryIndex) => {
    if (entry.kind === 'divider') {
      if (entry.id === RAIL_PLUGIN_VIEWS_DIVIDER_ID) return
      sections.push(section)
      // A user divider only splits the rail visually; its items stay in the
      // group the preceding built-in divider opened.
      const user = isUserDividerId(entry.id)
      section = {
        key: `${entry.id}-${entryIndex}`,
        sectionId: user
          ? section.sectionId
          : entry.id === 'workspace-contexts'
            ? 'conversations'
            : 'tools',
        label: user
          ? section.label
          : entry.id === 'workspace-contexts'
            ? conversationLabel
            : toolsLabel,
        dividerBefore: { id: entry.id, entryIndex, collapsed: Boolean(entry.collapsed) },
        items: []
      }
      return
    }
    section.items.push({ id: entry.id, entryIndex })
  })
  sections.push(section)
  return sections
}
