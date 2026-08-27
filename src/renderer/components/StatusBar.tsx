import { Bell, Download, FileQuestion, Folder, Pencil, Plus, Server } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { ContextBarSettingsPopover } from '@/components/ContextBarSettingsPopover'
import { GitBranchPicker } from '@/components/GitBranchPicker'
import { RemoteAccessPopover } from '@/components/RemoteAccessPopover'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { formatPath, useHomeDirectory } from '@/hooks/use-cwd'
import { getColorClasses } from '@/lib/colors'
import { cn } from '@/lib/utils'
import {
  useShowExitCode,
  useShowGitBranch,
  useShowGitStatus,
  useShowWorkingDirectory
} from '@/stores/context-bar-settings-store'
import { useActiveTerminal } from '@/stores/terminal-store'
import { useUpdateDownloaded, useUpdateVersion } from '@/stores/updater-store'
import type { Project } from '@/types/project'

interface StatusBarProps {
  project: Project | undefined
}

export function StatusBar({ project }: StatusBarProps): React.JSX.Element {
  const { t } = useTranslation('shell')
  const projectColor = project ? getColorClasses(project.color) : null
  const activeTerminal = useActiveTerminal()
  const homeDir = useHomeDirectory()

  // Context bar visibility settings
  const showGitBranch = useShowGitBranch()
  const showGitStatus = useShowGitStatus()
  const showWorkingDirectory = useShowWorkingDirectory()
  const showExitCode = useShowExitCode()

  // Updater state
  const updateDownloaded = useUpdateDownloaded()
  const updateVersion = useUpdateVersion()

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

      <div className="flex-1" />

      {/* Right side */}
      <div className="flex items-center gap-1">
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

        <StatusItem icon={<Bell size={14} />} />
        <ContextBarSettingsPopover />
      </div>
    </div>
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
