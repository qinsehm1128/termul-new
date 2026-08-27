import type { DetectedShells, ShellInfo } from '@shared/types/ipc.types'
import { Reorder } from 'framer-motion'
import { ChevronDown, GitBranch, Plus, Terminal as TerminalIcon, X } from 'lucide-react'
import {
  type FocusEvent,
  type KeyboardEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
  type WheelEvent
} from 'react'
import { useTranslation } from 'react-i18next'
import { pathBasename } from '@/components/lists'
import { shellApi } from '@/lib/api'
import { isPreferredShell } from '@/lib/shell-api'
import { cn } from '@/lib/utils'
import { useProjectStore } from '@/stores/project-store'
import { isConversationScopedTerminal, type Terminal } from '@/types/project'
import { Skeleton } from './ui/skeleton'
import { TabContextMenu } from './workspace/tab-context-menu'

interface TerminalTabBarProps {
  terminals: Terminal[]
  activeTerminalId: string
  onSelectTerminal: (id: string) => void
  onCloseTerminal: (id: string) => void
  onNewTerminal: () => void
  onNewTerminalWithShell?: (shell: ShellInfo) => void
  onRenameTerminal: (id: string, name: string) => void
  onReorderTerminals: (orderedIds: string[]) => void
  defaultShell?: string
}

export function TerminalTabBar({
  terminals,
  activeTerminalId,
  onSelectTerminal,
  onCloseTerminal,
  onNewTerminal,
  onNewTerminalWithShell,
  onRenameTerminal,
  onReorderTerminals,
  defaultShell
}: TerminalTabBarProps) {
  const { t } = useTranslation('shell')
  const [isDropdownOpen, setIsDropdownOpen] = useState(false)
  const [shells, setShells] = useState<DetectedShells | null>(null)
  const [loading, setLoading] = useState(true)
  const [hasOverflow, setHasOverflow] = useState(false)
  const dropdownRef = useRef<HTMLDivElement>(null)
  const tabsContainerRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const fetchShells = async () => {
      try {
        const result = await shellApi.getAvailableShells()
        if (result.success) {
          setShells(result.data)
        }
      } catch {
        setShells(null)
      } finally {
        setLoading(false)
      }
    }
    void fetchShells()
  }, [])

  useEffect(() => {
    const handleClickOutside = (e: globalThis.MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsDropdownOpen(false)
      }
    }

    if (isDropdownOpen) {
      document.addEventListener('mousedown', handleClickOutside)
    }

    return () => {
      document.removeEventListener('mousedown', handleClickOutside)
    }
  }, [isDropdownOpen])

  useEffect(() => {
    const checkOverflow = () => {
      if (tabsContainerRef.current) {
        const { scrollWidth, clientWidth } = tabsContainerRef.current
        setHasOverflow(scrollWidth > clientWidth)
      }
    }

    checkOverflow()
    window.addEventListener('resize', checkOverflow)
    return () => window.removeEventListener('resize', checkOverflow)
  }, [])

  const handleWheel = useCallback((e: WheelEvent<HTMLDivElement>) => {
    if (tabsContainerRef.current) {
      e.preventDefault()
      tabsContainerRef.current.scrollLeft += e.deltaY
    }
  }, [])

  const handleSelectShell = useCallback(
    (shell: ShellInfo) => {
      if (onNewTerminalWithShell) {
        onNewTerminalWithShell(shell)
      }
      setIsDropdownOpen(false)
    },
    [onNewTerminalWithShell]
  )

  const sortedShells = shells?.available?.slice().sort((a, b) => {
    if (defaultShell) {
      if (isPreferredShell(a, defaultShell)) return -1
      if (isPreferredShell(b, defaultShell)) return 1
    }
    return a.displayName.localeCompare(b.displayName)
  })

  return (
    <div className="flex h-8 items-center border-b border-border/70 bg-sidebar shadow-[inset_0_1px_0_hsl(var(--foreground)/0.025)]">
      <div className="relative flex items-center h-full min-w-0 shrink">
        <div
          ref={tabsContainerRef}
          onWheel={handleWheel}
          className="overflow-x-auto scrollbar-hide flex items-center h-full"
        >
          <Reorder.Group
            axis="x"
            values={terminals}
            onReorder={(reordered) => onReorderTerminals(reordered.map((t) => t.id))}
            className="flex items-center h-full"
          >
            {terminals.map((terminal) => (
              <Reorder.Item
                key={terminal.id}
                value={terminal}
                className="list-none h-full"
                whileDrag={{ scale: 1.02, boxShadow: '0 4px 12px rgba(0,0,0,0.15)' }}
              >
                <TerminalTab
                  terminal={terminal}
                  isActive={terminal.id === activeTerminalId}
                  onSelect={() => onSelectTerminal(terminal.id)}
                  onClose={() => onCloseTerminal(terminal.id)}
                  onRename={(name) => onRenameTerminal(terminal.id, name)}
                />
              </Reorder.Item>
            ))}
          </Reorder.Group>
        </div>

        {/* Gradient overlay when there are more tabs */}
        {hasOverflow && (
          <div className="pointer-events-none absolute right-0 top-0 h-full w-8 bg-gradient-to-l from-sidebar to-transparent" />
        )}
      </div>

      {/* Split Button: New Terminal */}
      <div ref={dropdownRef} className="relative flex items-center ml-1 shrink-0">
        <button
          onClick={onNewTerminal}
          className="flex h-7 w-7 items-center justify-center rounded-l-md border-r border-border/50 text-muted-foreground transition-colors duration-150 hover:bg-secondary hover:text-foreground"
          title={t('terminalTabs.newTerminalDefault')}
        >
          <Plus size={14} />
        </button>
        {onNewTerminalWithShell && (
          <>
            <button
              onClick={() => setIsDropdownOpen(!isDropdownOpen)}
              className="flex h-7 w-5 items-center justify-center rounded-r-md text-muted-foreground transition-colors duration-150 hover:bg-secondary hover:text-foreground"
              title={t('terminalTabs.selectShell')}
            >
              <ChevronDown size={14} />
            </button>

            {isDropdownOpen && (
              <div className="absolute left-0 top-full z-50 mt-1 w-48 overflow-hidden rounded-md border border-border/80 bg-popover shadow-[0_12px_36px_hsl(var(--background)/0.65),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]">
                {loading ? (
                  <div className="space-y-2 px-3 py-1">
                    <Skeleton className="h-8 w-full" />
                    <Skeleton className="h-8 w-full" />
                    <Skeleton className="h-8 w-full" />
                  </div>
                ) : sortedShells && sortedShells.length > 0 ? (
                  <div className="py-1">
                    {sortedShells.map((shell) => (
                      <button
                        key={shell.name}
                        onClick={() => handleSelectShell(shell)}
                        className={cn(
                          'flex w-full items-center gap-2 px-3 py-2 text-left text-sm transition-colors duration-150 hover:bg-secondary',
                          isPreferredShell(shell, defaultShell) && 'text-primary'
                        )}
                      >
                        <TerminalIcon size={14} />
                        <span>{shell.displayName}</span>
                        {isPreferredShell(shell, defaultShell) && (
                          <span className="ml-auto text-xs text-muted-foreground">
                            {t('terminalTabs.default')}
                          </span>
                        )}
                      </button>
                    ))}
                  </div>
                ) : (
                  <div className="px-3 py-2 text-sm text-muted-foreground">
                    {t('terminalTabs.noShells')}
                  </div>
                )}
              </div>
            )}
          </>
        )}
      </div>

      {/* Spacer to fill remaining width */}
      <div className="flex-1" />
    </div>
  )
}

interface TerminalTabProps {
  terminal: Terminal
  isActive: boolean
  onSelect: () => void
  onClose: () => void
  onRename: (name: string) => void
}

function TerminalTab({ terminal, isActive, onSelect, onClose, onRename }: TerminalTabProps) {
  const { t } = useTranslation('shell')
  const [isEditing, setIsEditing] = useState(false)
  const [editName, setEditName] = useState(terminal.name)
  const inputRef = useRef<HTMLInputElement>(null)

  // Worktree context: resolve the worktree associated with this specific terminal
  const projects = useProjectStore((state) => state.projects)
  const project = projects.find((p) => p.id === terminal.projectId)
  const terminalWorktree = terminal.worktreeId
    ? project?.worktrees?.find((w) => w.id === terminal.worktreeId)
    : null

  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus()
      inputRef.current.select()
    }
  }, [isEditing])

  const handleDoubleClick = useCallback(() => {
    setEditName(terminal.name)
    setIsEditing(true)
  }, [terminal.name])

  const handleSave = useCallback(() => {
    const trimmedName = editName.trim()
    if (trimmedName && trimmedName !== terminal.name) {
      onRename(trimmedName)
    }
    setIsEditing(false)
  }, [editName, terminal.name, onRename])

  const handleCancel = useCallback(() => {
    setEditName(terminal.name)
    setIsEditing(false)
  }, [terminal.name])

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        e.preventDefault()
        handleSave()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        handleCancel()
      }
    },
    [handleSave, handleCancel]
  )

  const handleBlur = useCallback(
    (_e: FocusEvent<HTMLInputElement>) => {
      handleSave()
    },
    [handleSave]
  )

  const handleRenameFromMenu = useCallback(() => {
    setEditName(terminal.name)
    setIsEditing(true)
  }, [terminal.name])

  return (
    <TabContextMenu kind="terminal" onClose={onClose} onRename={handleRenameFromMenu}>
      <div
        onClick={onSelect}
        className={cn(
          'group flex h-full min-w-[100px] cursor-pointer items-center border-r border-border/50 px-3 transition-colors duration-150',
          isActive
            ? 'z-[1] -mb-px bg-background text-foreground shadow-[inset_0_1px_0_0_hsl(var(--primary)/0.55)]'
            : 'text-muted-foreground hover:bg-secondary/40 hover:text-foreground'
        )}
      >
        <TerminalIcon size={14} className={cn('mr-2', isActive ? 'text-primary' : '')} />
        {isEditing ? (
          <input
            ref={inputRef}
            type="text"
            value={editName}
            onChange={(e) => setEditName(e.target.value)}
            onKeyDown={handleKeyDown}
            onBlur={handleBlur}
            onClick={(e) => e.stopPropagation()}
            className="text-2xs font-medium bg-transparent border-b border-primary outline-none w-full"
          />
        ) : (
          <>
            <span
              onDoubleClick={handleDoubleClick}
              className={cn(
                'max-w-[88px] truncate text-2xs font-medium',
                isActive && 'text-foreground'
              )}
            >
              {terminal.name}
            </span>
            {pathBasename(terminal.cwd) ? (
              <span className="ml-1 max-w-[64px] truncate text-3xs text-muted-foreground">
                {pathBasename(terminal.cwd)}
              </span>
            ) : null}
            {terminalWorktree && (
              <span className="ml-1.5 inline-flex items-center gap-0.5 rounded-sm bg-accent/10 px-1 py-0.5 text-4xs font-medium text-accent-foreground/80 leading-none">
                <GitBranch size={8} />
                <span className="max-w-[50px] truncate">{terminalWorktree.name}</span>
              </span>
            )}
          </>
        )}
        <button
          onClick={(e) => {
            e.stopPropagation()
            onClose()
          }}
          className="ml-auto rounded-md p-0.5 text-muted-foreground opacity-70 transition-opacity duration-150 hover:bg-secondary hover:text-foreground group-hover:opacity-100"
          aria-label={
            isConversationScopedTerminal(terminal)
              ? t('terminalTabs.closeView', { name: terminal.name })
              : t('terminalTabs.terminateProcess', { name: terminal.name })
          }
        >
          <X size={11} />
        </button>
      </div>
    </TabContextMenu>
  )
}
