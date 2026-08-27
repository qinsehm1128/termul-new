import {
  CalendarClock,
  FolderKanban,
  GitBranch,
  History,
  MessageSquarePlus,
  Network,
  Palette,
  SlidersHorizontal,
  SquareTerminal
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
import { TermulMark } from '@/components/TermulMark'
import { TitleBarShortcutsPopover } from '@/components/TitleBarShortcutsPopover'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { useUpdatePanelVisibility } from '@/hooks/use-app-settings'
import { isMac } from '@/lib/platform'
import { isTauriContext } from '@/lib/tauri-runtime'
import { cn } from '@/lib/utils'
import { useSSHPanelVisible } from '@/stores/ssh-panel-store'
import { useTerminalStore } from '@/stores/terminal-store'

const railButtonClass =
  'relative mx-1 flex h-9 w-9 items-center justify-center rounded-md text-muted-foreground transition-[color,background-color,box-shadow] duration-150 ease-[var(--ease-out)] before:absolute before:-left-1 before:h-4 before:w-px before:rounded-full before:bg-primary before:opacity-0 before:transition-opacity hover:bg-foreground/[0.045] hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring/80 aria-[pressed=true]:bg-foreground/[0.065] aria-[pressed=true]:text-foreground aria-[pressed=true]:shadow-[inset_0_1px_0_hsl(var(--foreground)/0.035)] aria-[pressed=true]:before:opacity-100 aria-[current=page]:bg-foreground/[0.065] aria-[current=page]:text-foreground aria-[current=page]:before:opacity-100 disabled:opacity-30'

function RailTooltip({
  label,
  disabled = false,
  children
}: {
  label: string
  disabled?: boolean
  children: React.ReactElement
}): React.JSX.Element {
  return (
    <Tooltip>
      <TooltipTrigger asChild data-rail-tooltip={label}>
        {disabled ? <span className="block leading-none">{children}</span> : children}
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

/**
 * Vertical activity rail (VSCode-style) that hosts the app's global actions.
 *
 * Layout:
 * - macOS: WorkspaceLayout renders a full-width titlebar zone above this rail;
 *   the brand row stays draggable for top-left window moves.
 * - Brand mark at the top, followed by a separator.
 * - Project section: project workspace, git changes/history, and SSH panel.
 * - Conversation section: independent chat workspace and scheduled tasks,
 *   separated from project actions to reduce accidental context switches.
 * - Bottom group (pinned via `mt-auto`): keyboard shortcuts, preferences,
 *   color themes. Sidebar/file-explorer visibility toggles moved to the
 *   titlebar strip (TitleBar / MacOsTitlebarStrip) beside the OS window
 *   controls.
 *
 * The SSH panel toggle preserves the persistence-aware updater, error-toast,
 * and accessible-label contracts that previously lived in the top title bar.
 * Sidebar/file-explorer visibility toggles now live in the titlebar strip.
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
  const isSSHPanelVisible = useSSHPanelVisible()
  const updatePanelVisibility = useUpdatePanelVisibility()
  const navigate = useNavigate()
  const location = useLocation()
  const isConversationsActive =
    location.pathname === '/conversations' || location.pathname.startsWith('/c/')
  const isTerminalsActive = location.pathname === '/terminals'
  const liveTerminalCount = useTerminalStore(
    (state) => state.terminals.filter((terminal) => Boolean(terminal.ptyId)).length
  )

  const handleToggleSSHPanel = async (e: React.MouseEvent<HTMLButtonElement>): Promise<void> => {
    e.stopPropagation()
    try {
      await updatePanelVisibility('sshPanelVisible', !isSSHPanelVisible)
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('activityRail.failedSsh'))
    }
  }

  return (
    <TooltipProvider delayDuration={120} skipDelayDuration={200}>
      <nav
        className="flex w-11 shrink-0 select-none flex-col items-center border-r border-sidebar-border/70 bg-sidebar shadow-[inset_-1px_0_0_hsl(var(--background)/0.35)]"
        aria-label={t('activityRail.globalActions')}
      >
        {/* Brand mark */}
        <div
          className="flex h-9 w-11 shrink-0 items-center justify-center text-foreground/90"
          data-tauri-drag-region={isMac ? true : undefined}
        >
          <TermulMark size={19} className="pointer-events-none" />
        </div>

        <div className="my-1 h-px w-4 bg-border/70" aria-hidden="true" />

        <fieldset
          aria-label={t('activityRail.projectActions')}
          data-activity-rail-section="projects"
          className="m-0 flex min-w-0 flex-col items-center gap-0.5 border-0 p-0"
        >
          <RailTooltip label={t('activityRail.projects')}>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                navigate('/')
              }}
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

          <RailTooltip label={t('activityRail.terminals')}>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                navigate(isTerminalsActive ? '/' : '/terminals')
              }}
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

          <RailTooltip
            disabled={!onOpenGitChanges || !canOpenGitChanges}
            label={
              canOpenGitChanges
                ? t('activityRail.gitChanges')
                : t('activityRail.gitChangesNeedsProject')
            }
          >
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                onOpenGitChanges?.()
              }}
              className={railButtonClass}
              aria-label={t('activityRail.openGitChanges')}
              disabled={!onOpenGitChanges || !canOpenGitChanges}
            >
              <GitBranch
                size={18}
                className={canOpenGitChanges ? 'text-muted-foreground' : 'text-muted-foreground/40'}
              />
            </button>
          </RailTooltip>

          <RailTooltip
            disabled={!onOpenGitHistory || !canOpenGitHistory}
            label={
              canOpenGitHistory
                ? t('activityRail.gitHistory')
                : t('activityRail.gitHistoryNeedsProject')
            }
          >
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                onOpenGitHistory?.()
              }}
              className={railButtonClass}
              aria-label={t('activityRail.openGitHistory')}
              disabled={!onOpenGitHistory || !canOpenGitHistory}
            >
              <History
                size={18}
                className={canOpenGitHistory ? 'text-muted-foreground' : 'text-muted-foreground/40'}
              />
            </button>
          </RailTooltip>

          <RailTooltip
            disabled={!isTauriContext()}
            label={
              isTauriContext() ? t('activityRail.toggleSsh') : t('activityRail.sshDesktopOnly')
            }
          >
            <button
              type="button"
              onClick={(e) => {
                void handleToggleSSHPanel(e)
              }}
              className={railButtonClass}
              aria-label={isSSHPanelVisible ? t('activityRail.hideSsh') : t('activityRail.showSsh')}
              aria-pressed={isSSHPanelVisible}
              disabled={!isTauriContext()}
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
        </fieldset>

        <div
          className="my-1.5 h-px w-5 bg-sidebar-border/80"
          data-activity-rail-divider="workspace-contexts"
          aria-hidden="true"
        />

        <fieldset
          aria-label={t('activityRail.conversationActions')}
          data-activity-rail-section="conversations"
          className="m-0 flex min-w-0 flex-col items-center border-0 p-0"
        >
          <RailTooltip label={t('activityRail.conversations')}>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                navigate(isConversationsActive ? '/' : '/conversations')
              }}
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

          <RailTooltip label={t('activityRail.scheduledTasks')}>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                navigate('/scheduled-tasks')
              }}
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
        </fieldset>

        <div className="mt-auto flex flex-col items-center pb-1">
          <RailTooltip label={t('titleBar.keyboardShortcuts')}>
            <TitleBarShortcutsPopover
              buttonClassName={railButtonClass}
              open={isShortcutsOpen}
              onOpenChange={onShortcutsOpenChange}
            />
          </RailTooltip>

          <RailTooltip label={t('activityRail.preferences')}>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                navigate('/preferences')
              }}
              className={railButtonClass}
              aria-label={t('activityRail.openPreferences')}
              aria-current={location.pathname === '/preferences' ? 'page' : undefined}
            >
              <SlidersHorizontal
                size={18}
                className={
                  location.pathname === '/preferences' ? 'text-foreground' : 'text-muted-foreground'
                }
              />
            </button>
          </RailTooltip>

          <RailTooltip disabled={!onToggleThemePicker} label={t('activityRail.colorThemes')}>
            <button
              type="button"
              onClick={
                onToggleThemePicker
                  ? (e) => {
                      e.stopPropagation()
                      onToggleThemePicker()
                    }
                  : undefined
              }
              className={railButtonClass}
              aria-label={t('activityRail.colorThemes')}
              aria-pressed={onToggleThemePicker ? isThemePickerOpen : undefined}
              aria-disabled={!onToggleThemePicker}
              disabled={!onToggleThemePicker}
            >
              <Palette
                size={18}
                className={isThemePickerOpen ? 'text-foreground' : 'text-muted-foreground'}
              />
            </button>
          </RailTooltip>
        </div>
      </nav>
    </TooltipProvider>
  )
}
