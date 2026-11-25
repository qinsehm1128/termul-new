import {
  AlertTriangle,
  Bell,
  Check,
  Download,
  FileQuestion,
  Folder,
  LifeBuoy,
  Pencil,
  Plus,
  RefreshCw,
  Server
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { ContextBarSettingsPopover } from '@/components/ContextBarSettingsPopover'
import { GitBranchPicker } from '@/components/GitBranchPicker'
import { RemoteAccessPopover } from '@/components/RemoteAccessPopover'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { PluginStatusItems } from '@/features/plugins/PluginStatusItems'
import { formatPath, useHomeDirectory } from '@/hooks/use-cwd'
import { useTerminalRecovery } from '@/hooks/use-terminal-recovery'
import { getColorClasses } from '@/lib/colors'
import { terminalBoardStatus } from '@/lib/terminal-board'
import { presentPendingUpdate, translateUpdateCopy } from '@/lib/updater-status'
import { cn } from '@/lib/utils'
import {
  useShowExitCode,
  useShowGitBranch,
  useShowGitStatus,
  useShowWorkingDirectory
} from '@/stores/context-bar-settings-store'
import { useActiveTerminal, useTerminalStore } from '@/stores/terminal-store'
import { usePendingUpdatePlan, useUpdateDownloaded, useUpdateVersion } from '@/stores/updater-store'
import type { Project } from '@/types/project'

interface StatusBarProps {
  project: Project | undefined
}

export function StatusBar({ project }: StatusBarProps): React.JSX.Element {
  const { t } = useTranslation('shell')
  const projectColor = project ? getColorClasses(project.color) : null
  const activeTerminal = useActiveTerminal()
  const homeDir = useHomeDirectory()
  const { recover, recovering } = useTerminalRecovery()

  // Context bar visibility settings
  const showGitBranch = useShowGitBranch()
  const showGitStatus = useShowGitStatus()
  const showWorkingDirectory = useShowWorkingDirectory()
  const showExitCode = useShowExitCode()

  // Updater state
  const updateDownloaded = useUpdateDownloaded()
  const updateVersion = useUpdateVersion()
  const pendingUpdatePlan = usePendingUpdatePlan()
  const pendingUpdate = presentPendingUpdate(pendingUpdatePlan, translateUpdateCopy)

  // Display terminal CWD if available, otherwise fall back to project path
  const displayPath = activeTerminal?.cwd || project?.path
  const formattedPath = displayPath ? formatPath(displayPath, homeDir) : undefined

  // Display terminal git branch if available, otherwise fall back to project gitBranch
  const gitBranch = activeTerminal?.gitBranch ?? project?.gitBranch

  // Git status from active terminal
  const gitStatus = activeTerminal?.gitStatus

  // Last command exit code from active terminal
  const lastExitCode = activeTerminal?.lastExitCode

  return (
    <div className="relative z-50 flex h-6 flex-shrink-0 select-none items-center border-t border-border bg-status-bar px-1.5 font-sans text-2xs text-status-bar-foreground shadow-[inset_0_1px_0_hsl(var(--foreground)/0.018)]">
      {/* Left side */}
      <div className="flex min-w-0 items-center gap-1">
        {project && (
          <>
            <StatusItem
              icon={
                <span className="flex items-center gap-1.5">
                  <span
                    className={cn('size-1.5 rounded-[1px]', projectColor?.bg)}
                    data-project-color={project.color}
                    aria-hidden="true"
                  />
                  <Server size={14} />
                </span>
              }
            >
              {project.name.toLowerCase().replace(/\s+/g, '-')}
            </StatusItem>

            {showGitBranch && displayPath && (
              <GitBranchPicker
                repoPath={displayPath}
                currentBranch={gitBranch}
                projectId={project.id}
                ahead={gitStatus?.ahead}
                behind={gitStatus?.behind}
              />
            )}

            {showGitStatus && gitStatus?.hasChanges && (
              <GitStatusIndicator
                modified={gitStatus.modified}
                staged={gitStatus.staged}
                untracked={gitStatus.untracked}
              />
            )}

            {showWorkingDirectory && formattedPath && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <div>
                    <StatusItem icon={<Folder size={14} />} className="opacity-80">
                      {formattedPath}
                    </StatusItem>
                  </div>
                </TooltipTrigger>
                <TooltipContent side="top" className="max-w-md break-all">
                  {displayPath}
                </TooltipContent>
              </Tooltip>
            )}
          </>
        )}
      </div>

      <div className="flex min-w-0 flex-1 justify-end overflow-hidden">
        <div className="min-w-0 max-w-[30vw] overflow-hidden">
          <PluginStatusItems projectId={project?.id} />
        </div>
      </div>

      {/* Built-in controls never shrink to make room for plugin status items. */}
      <div className="flex shrink-0 items-center gap-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              aria-label={t('recovery.button')}
              disabled={recovering}
              onClick={recover}
              className="flex h-5 items-center rounded-sm px-1.5 text-muted-foreground transition-[color,background-color] duration-150 ease-[var(--ease-out)] hover:bg-foreground/[0.045] hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-60"
            >
              <LifeBuoy size={14} className={recovering ? 'animate-spin' : undefined} />
            </button>
          </TooltipTrigger>
          <TooltipContent side="top" className="max-w-xs">
            {t('recovery.tooltip')}
          </TooltipContent>
        </Tooltip>
        <RemoteAccessPopover />

        {showExitCode && lastExitCode !== null && lastExitCode !== undefined && (
          <Tooltip>
            <TooltipTrigger asChild>
              <div>
                <StatusItem>
                  <span
                    className={cn(
                      'w-2 h-2 rounded-full mr-2',
                      lastExitCode === 0 ? 'bg-success' : 'bg-destructive'
                    )}
                  />
                  {t('statusBar.exit', { code: lastExitCode })}
                </StatusItem>
              </div>
            </TooltipTrigger>
            <TooltipContent side="top">
              {lastExitCode === 0
                ? t('statusBar.lastCommandSucceeded')
                : t('statusBar.lastCommandFailed', { code: lastExitCode })}
            </TooltipContent>
          </Tooltip>
        )}

        {pendingUpdate && (
          <Tooltip>
            <TooltipTrigger asChild>
              <div
                className="flex items-center"
                role="status"
                aria-label={pendingUpdate.label}
                data-update-plan-status={pendingUpdate.status}
              >
                <StatusItem
                  icon={
                    pendingUpdate.status === 'completed' ? (
                      <Check size={14} />
                    ) : pendingUpdate.status === 'reconciling' ? (
                      <RefreshCw size={14} className="animate-spin" />
                    ) : (
                      <AlertTriangle size={14} />
                    )
                  }
                  className={
                    pendingUpdate.tone === 'danger'
                      ? 'text-destructive'
                      : pendingUpdate.tone === 'warning'
                        ? 'text-warning'
                        : pendingUpdate.tone === 'success'
                          ? 'text-success'
                          : 'text-muted-foreground'
                  }
                />
              </div>
            </TooltipTrigger>
            <TooltipContent side="top" className="max-w-sm">
              {pendingUpdate.label}
            </TooltipContent>
          </Tooltip>
        )}

        {updateDownloaded && (
          <Tooltip>
            <TooltipTrigger asChild>
              <div className="flex items-center">
                <StatusItem icon={<Download size={14} />} className="text-success" />
              </div>
            </TooltipTrigger>
            <TooltipContent side="top">
              {t('statusBar.updateReady', { version: updateVersion })}
            </TooltipContent>
          </Tooltip>
        )}

        <StatusNotificationsPopover
          updateDownloaded={updateDownloaded}
          updateVersion={updateVersion}
          pendingUpdateLabel={pendingUpdate?.label ?? null}
        />
        <ContextBarSettingsPopover />
      </div>
    </div>
  )
}

interface StatusNotificationsPopoverProps {
  updateDownloaded: boolean
  updateVersion: string | null
  pendingUpdateLabel: string | null
}

function StatusNotificationsPopover({
  updateDownloaded,
  updateVersion,
  pendingUpdateLabel
}: StatusNotificationsPopoverProps): React.JSX.Element {
  const { t } = useTranslation('shell')
  const terminals = useTerminalStore((state) => state.terminals ?? [])
  const selectTerminal = useTerminalStore((state) => state.selectTerminal)

  const attentionTerminals = terminals.filter((terminal) => {
    const status = terminalBoardStatus(terminal)
    return status === 'attention' || status === 'disconnected'
  })

  const totalCount =
    attentionTerminals.length + (updateDownloaded ? 1 : 0) + (pendingUpdateLabel ? 1 : 0)

  return (
    <Popover>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <button
              type="button"
              aria-label={t('statusBar.notifications', { defaultValue: 'Notifications' })}
              className="relative flex h-5 items-center gap-1 rounded-sm px-1.5 text-muted-foreground transition-[color,background-color] duration-150 ease-[var(--ease-out)] hover:bg-foreground/[0.045] hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            >
              <Bell size={14} />
              {totalCount > 0 && (
                <>
                  <span
                    className="size-1.5 rounded-full bg-warning"
                    aria-hidden="true"
                    data-testid="status-bar-notification-dot"
                  />
                  <span className="font-mono text-3xs tabular-nums text-foreground">
                    {totalCount}
                  </span>
                </>
              )}
            </button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side="top">
          {t('statusBar.notifications', { defaultValue: 'Notifications' })}
        </TooltipContent>
      </Tooltip>
      <PopoverContent align="end" side="top" sideOffset={6} className="w-64 p-2">
        <div className="mb-1.5 flex items-center justify-between px-1">
          <span className="font-mono text-3xs font-medium uppercase tracking-wider text-muted-foreground">
            {t('statusBar.notifications', { defaultValue: 'Notifications' })}
          </span>
          {totalCount > 0 && (
            <span className="font-mono text-3xs tabular-nums text-muted-foreground">
              {totalCount}
            </span>
          )}
        </div>
        {totalCount === 0 ? (
          <p className="px-1 py-2 text-2xs text-muted-foreground">
            {t('statusBar.noNotifications', { defaultValue: 'No pending alerts' })}
          </p>
        ) : (
          <div className="space-y-1">
            {attentionTerminals.map((terminal) => {
              const status = terminalBoardStatus(terminal)
              return (
                <button
                  key={terminal.id}
                  type="button"
                  onClick={() => selectTerminal?.(terminal.id)}
                  className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-2xs text-foreground transition-colors hover:bg-secondary"
                >
                  <span
                    className={cn(
                      'size-1.5 shrink-0 rounded-full',
                      status === 'disconnected' ? 'bg-destructive' : 'bg-warning'
                    )}
                    aria-hidden="true"
                  />
                  <span className="truncate font-medium">{terminal.name}</span>
                  <span className="ml-auto font-mono text-3xs text-muted-foreground">{status}</span>
                </button>
              )
            })}
            {pendingUpdateLabel && (
              <div className="flex items-center gap-2 rounded-md bg-secondary/40 px-2 py-1.5 text-2xs text-foreground">
                <AlertTriangle size={12} className="shrink-0 text-warning" />
                <span className="truncate">{pendingUpdateLabel}</span>
              </div>
            )}
            {updateDownloaded && (
              <div className="flex items-center gap-2 rounded-md bg-secondary/40 px-2 py-1.5 text-2xs text-foreground">
                <Download size={12} className="shrink-0 text-success" />
                <span className="truncate">
                  {t('statusBar.updateReady', { version: updateVersion })}
                </span>
              </div>
            )}
          </div>
        )}
      </PopoverContent>
    </Popover>
  )
}

interface StatusItemProps {
  icon?: React.ReactNode
  children?: React.ReactNode
  className?: string
}

function StatusItem({ icon, children, className }: StatusItemProps): React.JSX.Element {
  return (
    <div
      className={cn(
        'flex h-5 cursor-pointer items-center rounded-sm px-1.5 transition-[color,background-color] duration-150 ease-[var(--ease-out)] hover:bg-foreground/[0.045] hover:text-foreground',
        className
      )}
    >
      {icon && <span className="mr-1.5">{icon}</span>}
      {children}
    </div>
  )
}

interface GitStatusIndicatorProps {
  modified: number
  staged: number
  untracked: number
}

function GitStatusIndicator({
  modified,
  staged,
  untracked
}: GitStatusIndicatorProps): React.JSX.Element | null {
  const { t } = useTranslation('shell')
  const items: React.ReactNode[] = []

  if (modified > 0) {
    items.push(
      <Tooltip key="modified">
        <TooltipTrigger asChild>
          <span className="flex items-center text-warning">
            <Pencil size={12} className="mr-0.5" />
            {modified}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top">{t('statusBar.modified', { count: modified })}</TooltipContent>
      </Tooltip>
    )
  }

  if (staged > 0) {
    items.push(
      <Tooltip key="staged">
        <TooltipTrigger asChild>
          <span className="flex items-center text-success">
            <Plus size={12} className="mr-0.5" />
            {staged}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top">{t('statusBar.staged', { count: staged })}</TooltipContent>
      </Tooltip>
    )
  }

  if (untracked > 0) {
    items.push(
      <Tooltip key="untracked">
        <TooltipTrigger asChild>
          <span className="flex items-center text-muted-foreground">
            <FileQuestion size={12} className="mr-0.5" />
            {untracked}
          </span>
        </TooltipTrigger>
        <TooltipContent side="top">{t('statusBar.untracked', { count: untracked })}</TooltipContent>
      </Tooltip>
    )
  }

  if (items.length === 0) return null

  return (
    <div className="flex h-5 items-center gap-2 rounded-sm px-1.5 transition-colors hover:bg-secondary">
      {items}
    </div>
  )
}
