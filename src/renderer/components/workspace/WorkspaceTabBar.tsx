import type { DetectedShells, ShellInfo } from '@shared/types/ipc.types'
import {
  GitBranch,
  Globe,
  History,
  Loader2,
  Maximize2,
  Minimize2,
  Terminal as TerminalIcon,
  X as XIcon
} from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useShallow } from 'zustand/shallow'
import { AgentIcon } from '@/components/agents/AgentIcon'
import { AgentBadge } from '@/components/chat/AgentBadge'
import { AgentConnectionLamp } from '@/components/chat/AgentConnectionLamp'
import { isAgentConnected } from '@/components/chat/is-agent-connected'
import { pathBasename } from '@/components/lists'
import { Skeleton } from '@/components/ui/skeleton'
import { usePaneDnd } from '@/hooks/use-pane-dnd'
import { clipboardApi, shellApi } from '@/lib/api'
import { browserTabHide, browserTabShow } from '@/lib/browser-api'
import { isPreferredShell } from '@/lib/shell-api'
import { cn } from '@/lib/utils'
import { useAcpStore, useAgentIdentity } from '@/stores/acp-store'
import { useAnnotationStore } from '@/stores/annotation-store'
import { useBrowserSessionStore } from '@/stores/browser-session-store'
import { useEditorStore } from '@/stores/editor-store'
import { type GitStatusState, useGitStatusStore } from '@/stores/git-status-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { AgentChatTab, WorkspaceTab } from '@/stores/workspace-store'
import { editorTabId, useLeafCount, useWorkspaceStore } from '@/stores/workspace-store'
import { isConversationScopedTerminal, type Terminal } from '@/types/project'
import type { TabReorderPosition } from '@/types/workspace.types'
import { EditorTab } from './EditorTab'
import { TabContextMenu } from './tab-context-menu'

// Helper to compute drop position from mouse coordinates
function computeTabPosition(target: HTMLElement, clientX: number): TabReorderPosition {
  const rect = target.getBoundingClientRect()
  const x = clientX - rect.left
  const halfWidth = rect.width / 2
  return x < halfWidth ? 'before' : 'after'
}

const TAB_SURFACE =
  'group relative flex h-full min-w-[100px] cursor-grab select-none items-center px-2.5 transition-colors duration-150 ease-[var(--ease-out)]'

function tabToneClass(isActive: boolean, isDragging: boolean): string {
  return cn(
    TAB_SURFACE,
    isActive
      ? 'z-[1] -mb-px bg-background text-foreground shadow-[inset_0_1px_0_0_hsl(var(--primary)/0.55)]'
      : 'text-muted-foreground hover:bg-secondary/40 hover:text-foreground',
    isDragging && 'opacity-50'
  )
}

function TabDropMark({
  isDropTarget,
  dropPosition
}: {
  isDropTarget: boolean
  dropPosition: TabReorderPosition | null
}): React.JSX.Element | null {
  if (!isDropTarget || !dropPosition) return null
  return (
    <div
      className={cn(
        'absolute top-1.5 bottom-1.5 z-10 w-px bg-primary',
        dropPosition === 'before' ? 'left-0' : 'right-0'
      )}
    />
  )
}

function TabCloseButton({
  onClose,
  disabled = false,
  isActive,
  spinning = false,
  ariaLabel
}: {
  onClose: () => void
  disabled?: boolean
  isActive: boolean
  spinning?: boolean
  ariaLabel?: string
}): React.JSX.Element {
  return (
    <button
      type="button"
      aria-label={ariaLabel}
      onClick={(e) => {
        e.stopPropagation()
        if (!disabled) onClose()
      }}
      disabled={disabled}
      className={cn(
        'ml-auto flex h-4 w-4 shrink-0 items-center justify-center text-muted-foreground transition-opacity duration-150 ease-[var(--ease-out)] hover:bg-secondary hover:text-foreground disabled:cursor-wait',
        isActive || spinning ? 'opacity-70 hover:opacity-100' : 'opacity-0 group-hover:opacity-100'
      )}
    >
      {spinning ? <Loader2 size={11} className="animate-spin" /> : <XIcon size={11} />}
    </button>
  )
}

function TabLiveMark({
  attention,
  activity,
  running
}: {
  attention?: boolean
  activity?: boolean
  running?: boolean
}): React.JSX.Element | null {
  if (attention) {
    return <span className="mr-0.5 h-1.5 w-1.5 shrink-0 bg-warning" aria-hidden />
  }
  if (activity) {
    return <span className="mr-0.5 h-1.5 w-1.5 shrink-0 bg-primary" aria-hidden />
  }
  if (running) {
    return <span className="mr-0.5 h-1.5 w-1.5 shrink-0 bg-primary/40" aria-hidden />
  }
  return null
}

// Inline TerminalTab matching the style from TerminalTabBar

interface TerminalTabInlineProps {
  terminal: Terminal
  isActive: boolean
  isDragging: boolean
  isDropTarget: boolean
  dropPosition: TabReorderPosition | null
  isClosing?: boolean
  onSelect: () => void
  onClose: () => void
  onRename: (name: string) => void
  onDragStart: (e: React.DragEvent) => void
  onDragOver: (e: React.DragEvent) => void
  onDragLeave: () => void
  onDrop: (e: React.DragEvent) => void
}

function TerminalTabInline({
  terminal,
  isActive,
  isDragging,
  isDropTarget,
  dropPosition,
  isClosing = false,
  onSelect,
  onClose,
  onRename,
  onDragStart,
  onDragOver,
  onDragLeave,
  onDrop
}: TerminalTabInlineProps): React.JSX.Element {
  const { t } = useTranslation('workspace')
  const folder = pathBasename(terminal.cwd)
  const [isEditing, setIsEditing] = useState(false)
  const [editName, setEditName] = useState(terminal.name)
  const inputRef = useRef<HTMLInputElement>(null)

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

  const handleRenameFromMenu = useCallback(() => {
    setEditName(terminal.name)
    setIsEditing(true)
  }, [terminal.name])

  return (
    <TabContextMenu
      kind="terminal"
      onClose={onClose}
      onRename={handleRenameFromMenu}
      isClosing={isClosing}
    >
      <div
        draggable={!isEditing}
        onDragStart={onDragStart}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onDrop={onDrop}
        onClick={onSelect}
        onAuxClick={(e) => {
          if (e.button !== 1) return
          e.preventDefault()
          e.stopPropagation()
          if (!isClosing) {
            onClose()
          }
        }}
        className={cn(tabToneClass(isActive, isDragging), 'border-r border-border/50')}
      >
        <TabDropMark isDropTarget={isDropTarget} dropPosition={dropPosition} />
        <TabLiveMark
          attention={terminal.needsAttention}
          activity={terminal.hasActivity}
          running={terminal.healthStatus === 'running'}
        />

        {terminal.kind === 'agent' && terminal.agentId ? (
          <AgentIcon
            agentId={terminal.agentId}
            name={terminal.agentName}
            className="mr-2 size-3.5"
          />
        ) : (
          <TerminalIcon size={14} className={cn('mr-2', isActive ? 'text-primary' : '')} />
        )}
        {isEditing ? (
          <input
            ref={inputRef}
            type="text"
            value={editName}
            onChange={(e) => setEditName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                handleSave()
              } else if (e.key === 'Escape') {
                e.preventDefault()
                handleCancel()
              }
            }}
            onBlur={handleSave}
            onClick={(e) => e.stopPropagation()}
            className="text-2xs font-medium bg-transparent border-b border-primary outline-none w-full"
          />
        ) : (
          <span
            onDoubleClick={handleDoubleClick}
            className={cn(
              'max-w-[88px] truncate text-2xs font-medium',
              isActive && 'text-foreground'
            )}
          >
            {terminal.name}
          </span>
        )}
        {folder ? (
          <span className="ml-1 max-w-[64px] truncate text-3xs text-muted-foreground">
            {folder}
          </span>
        ) : null}
        <TabCloseButton
          onClose={onClose}
          disabled={isClosing}
          isActive={isActive}
          spinning={isClosing}
          ariaLabel={
            isConversationScopedTerminal(terminal)
              ? t('tabs.closeView', { name: terminal.name })
              : t('tabs.terminateProcess', { name: terminal.name })
          }
        />
      </div>
    </TabContextMenu>
  )
}

interface EditorTabWrapperProps {
  tab: { type: 'editor'; id: string; filePath: string }
  isActive: boolean
  isDragging: boolean
  isDropTarget: boolean
  dropPosition: TabReorderPosition | null
  onSelect: () => void
  onClose: () => void
  onCloseOthers: () => void
  onCloseAll: () => void
  onCopyPath: () => void
  onDragStart: (e: React.DragEvent) => void
  onDragOver: (e: React.DragEvent) => void
  onDragLeave: () => void
  onDrop: (e: React.DragEvent) => void
}

function EditorTabWrapper({
  tab,
  isActive,
  isDragging,
  isDropTarget,
  dropPosition,
  onSelect,
  onClose,
  onCloseOthers,
  onCloseAll,
  onCopyPath,
  onDragStart,
  onDragOver,
  onDragLeave,
  onDrop
}: EditorTabWrapperProps): React.JSX.Element {
  const { isDirty, operationStatus } = useEditorStore(
    useShallow((state) => {
      const file = state.openFiles.get(tab.filePath)
      return {
        isDirty: file?.isDirty ?? false,
        operationStatus: file?.operationStatus ?? 'idle'
      }
    })
  )
  return (
    <div
      draggable
      onDragStart={onDragStart}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
      className={cn(
        'relative h-full transition-colors duration-150 ease-out',
        '[&_.border-b-2]:border-b-transparent [&_.border-b-primary]:border-b-transparent [&_.border-r]:border-border/50',
        isActive && 'z-[1] -mb-px shadow-[inset_0_1px_0_0_hsl(var(--primary)/0.55)]',
        isDragging && 'opacity-50'
      )}
    >
      <TabDropMark isDropTarget={isDropTarget} dropPosition={dropPosition} />
      <EditorTab
        filePath={tab.filePath}
        isActive={isActive}
        isDirty={isDirty}
        operationStatus={operationStatus}
        onSelect={onSelect}
        onClose={onClose}
        onCloseOthers={onCloseOthers}
        onCloseAll={onCloseAll}
        onCopyPath={onCopyPath}
      />
    </div>
  )
}

interface BrowserTabInlineProps {
  tab: { type: 'browser'; id: string; browserTabId: string }
  isActive: boolean
  isDragging: boolean
  isDropTarget: boolean
  dropPosition: TabReorderPosition | null
  onSelect: () => void
  onClose: () => void
  onDragStart: (e: React.DragEvent) => void
  onDragOver: (e: React.DragEvent) => void
  onDragLeave: () => void
  onDrop: (e: React.DragEvent) => void
}

function BrowserTabInline({
  tab,
  isActive,
  isDragging,
  isDropTarget,
  dropPosition,
  onSelect,
  onClose,
  onDragStart,
  onDragOver,
  onDragLeave,
  onDrop
}: BrowserTabInlineProps): React.JSX.Element {
  const { t } = useTranslation('workspace')
  const browserTab = useBrowserSessionStore((state) => state.getTab(tab.browserTabId))
  const label = (() => {
    if (!browserTab) return t('tabs.browser')
    if (browserTab.title.trim()) return browserTab.title.trim()
    if (browserTab.url) {
      try {
        const parsed = new URL(browserTab.url)
        return parsed.host || parsed.hostname || browserTab.url
      } catch {
        return browserTab.url.replace(/^https?:\/\//, '').split('/')[0] || t('tabs.browser')
      }
    }
    return t('tabs.browser')
  })()

  return (
    <TabContextMenu kind="browser" onClose={onClose}>
      <div
        draggable
        onDragStart={onDragStart}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onDrop={onDrop}
        onClick={onSelect}
        onAuxClick={(e) => {
          if (e.button !== 1) return
          e.preventDefault()
          e.stopPropagation()
          onClose()
        }}
        className={cn(tabToneClass(isActive, isDragging), 'border-r border-border/50')}
      >
        <TabDropMark isDropTarget={isDropTarget} dropPosition={dropPosition} />

        <Globe size={14} className={cn('mr-2', isActive ? 'text-primary' : '')} />
        <span className={cn('text-2xs font-medium truncate', isActive && 'text-foreground')}>
          {label}
        </span>
        <TabCloseButton onClose={onClose} isActive={isActive} />
      </div>
    </TabContextMenu>
  )
}

function GitTabInline({
  tab,
  isActive,
  isDragging,
  isDropTarget,
  dropPosition,
  onSelect,
  onClose,
  onDragStart,
  onDragOver,
  onDragLeave,
  onDrop
}: {
  tab: { type: 'git'; id: string; cwd: string }
  isActive: boolean
  isDragging: boolean
  isDropTarget: boolean
  dropPosition: TabReorderPosition | null
  onSelect: () => void
  onClose: () => void
  onDragStart: (e: React.DragEvent) => void
  onDragOver: (e: React.DragEvent) => void
  onDragLeave: () => void
  onDrop: (e: React.DragEvent) => void
}) {
  const { t } = useTranslation('workspace')
  const totalChanges = useGitStatusStore(
    (state: GitStatusState) => (state.statuses[tab.cwd] || []).length
  )

  return (
    <TabContextMenu kind="git" onClose={onClose}>
      <div
        draggable
        onDragStart={onDragStart}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onDrop={onDrop}
        onClick={onSelect}
        className={cn(
          tabToneClass(isActive, isDragging),
          'max-w-[200px] min-w-[120px] gap-2 border-r border-border/50'
        )}
      >
        <TabDropMark isDropTarget={isDropTarget} dropPosition={dropPosition} />
        <GitBranch size={14} className={isActive ? 'text-primary' : ''} />
        <span className="truncate text-2xs font-medium flex-1">{t('tabs.gitChanges')}</span>
        {totalChanges > 0 && (
          <span
            className={cn(
              'min-w-[1ch] text-3xs font-medium tabular-nums',
              isActive ? 'text-primary' : 'text-muted-foreground'
            )}
          >
            {totalChanges}
          </span>
        )}
        <TabCloseButton onClose={onClose} isActive={isActive} />
      </div>
    </TabContextMenu>
  )
}

function GitHistoryTabInline({
  tab: _tab,
  isActive,
  isDragging,
  isDropTarget,
  dropPosition,
  onSelect,
  onClose,
  onDragStart,
  onDragOver,
  onDragLeave,
  onDrop
}: {
  tab: { type: 'git-history'; id: string; cwd: string }
  isActive: boolean
  isDragging: boolean
  isDropTarget: boolean
  dropPosition: TabReorderPosition | null
  onSelect: () => void
  onClose: () => void
  onDragStart: (e: React.DragEvent) => void
  onDragOver: (e: React.DragEvent) => void
  onDragLeave: () => void
  onDrop: (e: React.DragEvent) => void
}) {
  const { t } = useTranslation('workspace')
  return (
    <TabContextMenu kind="git-history" onClose={onClose}>
      <div
        draggable
        onDragStart={onDragStart}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onDrop={onDrop}
        onClick={onSelect}
        className={cn(
          tabToneClass(isActive, isDragging),
          'max-w-[200px] min-w-[120px] gap-2 border-r border-border/50'
        )}
      >
        <TabDropMark isDropTarget={isDropTarget} dropPosition={dropPosition} />
        <History size={14} className={isActive ? 'text-primary' : ''} />
        <span className="truncate text-2xs font-medium flex-1">{t('tabs.gitHistory')}</span>
        <TabCloseButton onClose={onClose} isActive={isActive} />
      </div>
    </TabContextMenu>
  )
}

function AgentChatTabInline({
  tab,
  isActive,
  isDragging,
  isDropTarget,
  dropPosition,
  onSelect,
  onClose,
  onDragStart,
  onDragOver,
  onDragLeave,
  onDrop
}: {
  tab: AgentChatTab
  isActive: boolean
  isDragging: boolean
  isDropTarget: boolean
  dropPosition: TabReorderPosition | null
  onSelect: () => void
  onClose: () => void
  onDragStart: (e: React.DragEvent) => void
  onDragOver: (e: React.DragEvent) => void
  onDragLeave: () => void
  onDrop: (e: React.DragEvent) => void
}) {
  const { t } = useTranslation('workspace')
  const session = useAcpStore((state) => {
    if (tab.sessionId) return state.sessions[tab.sessionId]
    return Object.values(state.sessions).find(
      (candidate) => candidate.conversationId === tab.conversationId
    )
  })
  const agentStatus = useAcpStore((s) => (session ? s.agentStatus[session.agentId] : undefined))
  const isLaunchingSession = useAcpStore((s) =>
    session ? Boolean(s.launchingSessionIds[session.id]) : false
  )
  const { name: agentName } = useAgentIdentity(session?.agentId ?? null)
  // The persisted index entry carries the effective title (agent-pushed title,
  // first-message derivation, or "Untitled Chat N"). `session.title` stays null
  // until an event sets it, so fall through to the index entry for the label.
  const indexTitle = useAcpStore((state) => {
    if (tab.conversationId) {
      return (
        state.sessionIndex.find((entry) => entry.conversationId === tab.conversationId)?.title ??
        null
      )
    }
    return state.sessionIndex.find((entry) => entry.id === tab.sessionId)?.title ?? null
  })
  // Treat in-flight launcher handoff as connected so we don't flash a red
  // disconnected lamp on the optimistic placeholder chat.
  const connected = isLaunchingSession || isAgentConnected(session, agentStatus)
  const isClosed = session?.status === 'closed'
  const tabLabel = session?.title ?? indexTitle ?? agentName ?? t('tabs.agentChat')

  return (
    <TabContextMenu kind="agent-chat" onClose={onClose}>
      <div
        draggable
        onDragStart={onDragStart}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onDrop={onDrop}
        onClick={onSelect}
        aria-label={`${tabLabel}, ${connected ? t('tabs.connected') : t('tabs.disconnected')}`}
        className={cn(
          tabToneClass(isActive, isDragging),
          'max-w-[200px] min-w-[120px] gap-1.5 border-r border-border/50'
        )}
      >
        <TabDropMark isDropTarget={isDropTarget} dropPosition={dropPosition} />
        {session ? (
          <>
            <AgentBadge
              agentId={session.agentId}
              showName={false}
              iconSize={14}
              className="shrink-0"
            />
            <span
              className={cn(
                'min-w-0 flex-1 truncate text-2xs font-medium',
                isClosed && 'line-through opacity-60',
                isActive ? 'text-foreground' : 'text-inherit'
              )}
            >
              {tabLabel}
            </span>
            <AgentConnectionLamp connected={connected} />
          </>
        ) : (
          <span className="truncate text-2xs font-medium flex-1">{t('tabs.agentChat')}</span>
        )}
        <TabCloseButton onClose={onClose} isActive={isActive} />
      </div>
    </TabContextMenu>
  )
}

interface WorkspaceTabBarProps {
  paneId: string
  tabs: WorkspaceTab[]
  activeTabId: string | null
  closingTerminalIds?: string[]
  onAddTerminal?: (shell?: ShellInfo) => void
  onAddBrowserTab?: () => void
  onCloseTerminal?: (id: string, tabId: string) => void
  onRenameTerminal?: (id: string, name: string) => void
  onCloseEditorTab?: (filePath: string) => void
  defaultShell?: string
}

export function WorkspaceTabBar({
  paneId,
  tabs,
  activeTabId,
  closingTerminalIds = [],
  onAddTerminal,
  onAddBrowserTab,
  onCloseTerminal,
  onRenameTerminal,
  onCloseEditorTab,
  defaultShell
}: WorkspaceTabBarProps): React.JSX.Element {
  const { t } = useTranslation('workspace')
  const { setActiveTab, setActivePane, fullscreenPaneId, togglePaneFullscreen } = useWorkspaceStore(
    useShallow((state) => ({
      setActiveTab: state.setActiveTab,
      setActivePane: state.setActivePane,
      fullscreenPaneId: state.fullscreenPaneId,
      togglePaneFullscreen: state.togglePaneFullscreen
    }))
  )
  const leafCount = useLeafCount()
  const {
    startTabDrag,
    dragPayload,
    reorderPreview,
    setReorderPreview,
    clearReorderPreview,
    handleTabReorder
  } = usePaneDnd()

  const [isTerminalMenuOpen, setIsTerminalMenuOpen] = useState(false)
  const [shells, setShells] = useState<DetectedShells | null>(null)
  const [loading, setLoading] = useState(true)
  const [hasOverflow, setHasOverflow] = useState(false)
  const terminalMenuRef = useRef<HTMLDivElement>(null)
  const tabsContainerRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const fetchShells = async (): Promise<void> => {
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
    const handleClickOutside = (e: globalThis.MouseEvent): void => {
      if (terminalMenuRef.current && !terminalMenuRef.current.contains(e.target as Node)) {
        setIsTerminalMenuOpen(false)
      }
    }
    if (isTerminalMenuOpen) {
      document.addEventListener('mousedown', handleClickOutside)
    }
    return () => {
      document.removeEventListener('mousedown', handleClickOutside)
    }
  }, [isTerminalMenuOpen])

  // biome-ignore lint/correctness/useExhaustiveDependencies: tabs.length intentionally retriggers overflow checks
  useEffect(() => {
    const checkOverflow = (): void => {
      if (tabsContainerRef.current) {
        const { scrollWidth, clientWidth } = tabsContainerRef.current
        setHasOverflow(scrollWidth > clientWidth)
      }
    }
    checkOverflow()
    window.addEventListener('resize', checkOverflow)
    return () => window.removeEventListener('resize', checkOverflow)
  }, [tabs.length])

  // Native child webviews paint above the DOM, so the terminal popover would be
  // obscured unless we temporarily hide browser webviews while the menu is open.
  useEffect(() => {
    const browserTabs = tabs.filter(
      (tab): tab is WorkspaceTab & { type: 'browser'; browserTabId: string } =>
        tab.type === 'browser'
    )
    if (browserTabs.length === 0) return

    const hideAll = (tabsToHide: Array<{ browserTabId: string }>): void => {
      for (const tab of tabsToHide) {
        void browserTabHide(tab.browserTabId).catch(console.error)
      }
    }

    const showActive = (activeBrowserTab?: { browserTabId: string }): void => {
      if (activeBrowserTab) {
        void browserTabShow(activeBrowserTab.browserTabId).catch(console.error)
      }
    }

    if (isTerminalMenuOpen) {
      hideAll(browserTabs)
      return
    }

    const activeBrowserTab = browserTabs.find((tab) => tab.id === activeTabId)
    showActive(activeBrowserTab)
  }, [isTerminalMenuOpen, tabs, activeTabId])

  const handleWheel = useCallback((e: React.WheelEvent<HTMLDivElement>) => {
    if (tabsContainerRef.current) {
      e.preventDefault()
      tabsContainerRef.current.scrollLeft += e.deltaY
    }
  }, [])

  const handleSelectShell = useCallback(
    (shell: ShellInfo) => {
      if (onAddTerminal) {
        onAddTerminal(shell)
      }
      setIsTerminalMenuOpen(false)
    },
    [onAddTerminal]
  )

  const handleCloseEditorTab = useCallback(
    (filePath: string) => {
      const operationStatus =
        useEditorStore.getState().openFiles.get(filePath)?.operationStatus ?? 'idle'
      if (operationStatus === 'saving' || operationStatus === 'reloading') {
        return
      }

      if (onCloseEditorTab) {
        onCloseEditorTab(filePath)
      } else {
        // Fallback: close from store directly
        const didClose = useEditorStore.getState().closeFileIfIdle(filePath)
        if (didClose) {
          useWorkspaceStore.getState().closeTab(paneId, editorTabId(filePath))
        }
      }
    },
    [onCloseEditorTab, paneId]
  )

  const handleCloseOtherEditorTabs = useCallback(
    (filePath: string) => {
      const editorTabs = tabs.filter(
        (t): t is WorkspaceTab & { type: 'editor' } =>
          t.type === 'editor' && t.filePath !== filePath
      )
      for (const tab of editorTabs) {
        handleCloseEditorTab(tab.filePath)
      }
    },
    [tabs, handleCloseEditorTab]
  )

  const handleCloseAllEditorTabs = useCallback(() => {
    const editorTabs = tabs.filter(
      (t): t is WorkspaceTab & { type: 'editor' } => t.type === 'editor'
    )
    for (const tab of editorTabs) {
      handleCloseEditorTab(tab.filePath)
    }
  }, [tabs, handleCloseEditorTab])

  const handleTabDragStart = useCallback(
    (tabId: string, e: React.DragEvent) => {
      startTabDrag(tabId, paneId, e)
    },
    [startTabDrag, paneId]
  )

  const handleTabDragOver = useCallback(
    (tabId: string, e: React.DragEvent) => {
      e.preventDefault()
      e.dataTransfer.dropEffect = 'move'

      if (dragPayload?.type !== 'tab') return
      if (dragPayload.sourcePaneId !== paneId) return

      const position = computeTabPosition(e.currentTarget as HTMLElement, e.clientX)
      setReorderPreview(paneId, tabId, position)
    },
    [dragPayload, paneId, setReorderPreview]
  )

  const handleTabDragLeave = useCallback(() => {
    // Only clear if we're not entering a child element
    // This is handled by the individual tab components
  }, [])

  const handleContainerDragLeave = useCallback(
    (e: React.DragEvent) => {
      // Only clear preview if actually leaving the container (not moving to child)
      const relatedTarget = e.relatedTarget as Node | null
      if (relatedTarget && e.currentTarget.contains(relatedTarget)) {
        return
      }
      clearReorderPreview()
    },
    [clearReorderPreview]
  )

  const handleTabDrop = useCallback(
    (tabId: string, e: React.DragEvent) => {
      // Only prevent/stop if this is a same-pane tab reorder
      // Otherwise, let the event bubble for cross-pane drops
      if (dragPayload?.type !== 'tab' || dragPayload.sourcePaneId !== paneId) {
        return
      }

      e.preventDefault()
      e.stopPropagation()

      const position = computeTabPosition(e.currentTarget as HTMLElement, e.clientX)
      handleTabReorder(paneId, tabId, position)
    },
    [dragPayload, paneId, handleTabReorder]
  )

  const sortedShells = shells?.available?.slice().sort((a, b) => {
    if (defaultShell) {
      if (isPreferredShell(a, defaultShell)) return -1
      if (isPreferredShell(b, defaultShell)) return 1
    }
    return a.displayName.localeCompare(b.displayName)
  })

  const terminalStoreTerminals = useTerminalStore(useShallow((state) => state.terminals))
  const isFullscreenPane = fullscreenPaneId === paneId

  // Check if this tab is being dragged
  const isTabDragging = (tabId: string): boolean =>
    dragPayload?.type === 'tab' && dragPayload.tabId === tabId

  // Check if this tab is a drop target
  const isTabDropTarget = (
    tabId: string
  ): { isTarget: boolean; position: TabReorderPosition | null } => {
    if (!reorderPreview || reorderPreview.paneId !== paneId) {
      return { isTarget: false, position: null }
    }
    if (reorderPreview.targetTabId === tabId) {
      return { isTarget: true, position: reorderPreview.position }
    }
    return { isTarget: false, position: null }
  }

  return (
    <div
      className="flex h-8 items-center border-b border-border/70 bg-sidebar"
      onDragOver={(e) => {
        e.preventDefault()
        e.dataTransfer.dropEffect = 'move'
      }}
    >
      <div className="relative flex items-center h-full min-w-0 flex-1 overflow-hidden">
        <div
          ref={tabsContainerRef}
          onWheel={handleWheel}
          onDragLeave={handleContainerDragLeave}
          className="overflow-x-auto scrollbar-hide flex items-center h-full min-w-0 flex-1"
        >
          <div className="flex items-center h-full min-w-max">
            {tabs.map((tab) => {
              const dragging = isTabDragging(tab.id)
              const { isTarget, position } = isTabDropTarget(tab.id)

              return (
                <div key={tab.id} className="list-none h-full">
                  {tab.type === 'terminal' ? (
                    (() => {
                      const terminal = terminalStoreTerminals.find((t) => t.id === tab.terminalId)
                      if (!terminal) return null
                      return (
                        <TerminalTabInline
                          terminal={terminal}
                          isActive={tab.id === activeTabId}
                          isDragging={dragging}
                          isDropTarget={isTarget}
                          dropPosition={position}
                          isClosing={closingTerminalIds.includes(tab.terminalId)}
                          onSelect={() => {
                            setActiveTab(paneId, tab.id)
                            setActivePane(paneId)
                          }}
                          onClose={() => {
                            if (onCloseTerminal) onCloseTerminal(tab.terminalId, tab.id)
                          }}
                          onRename={(name) => {
                            if (onRenameTerminal) onRenameTerminal(tab.terminalId, name)
                          }}
                          onDragStart={(e) => handleTabDragStart(tab.id, e)}
                          onDragOver={(e) => handleTabDragOver(tab.id, e)}
                          onDragLeave={handleTabDragLeave}
                          onDrop={(e) => handleTabDrop(tab.id, e)}
                        />
                      )
                    })()
                  ) : tab.type === 'editor' ? (
                    <EditorTabWrapper
                      tab={tab as { type: 'editor'; id: string; filePath: string }}
                      isActive={tab.id === activeTabId}
                      isDragging={dragging}
                      isDropTarget={isTarget}
                      dropPosition={position}
                      onSelect={() => {
                        setActiveTab(paneId, tab.id)
                        setActivePane(paneId)
                      }}
                      onClose={() => handleCloseEditorTab(tab.filePath)}
                      onCloseOthers={() => handleCloseOtherEditorTabs(tab.filePath)}
                      onCloseAll={handleCloseAllEditorTabs}
                      onCopyPath={() => void clipboardApi.writeText(tab.filePath)}
                      onDragStart={(e) => handleTabDragStart(tab.id, e)}
                      onDragOver={(e) => handleTabDragOver(tab.id, e)}
                      onDragLeave={handleTabDragLeave}
                      onDrop={(e) => handleTabDrop(tab.id, e)}
                    />
                  ) : tab.type === 'git' ? (
                    <GitTabInline
                      tab={tab as { type: 'git'; id: string; cwd: string }}
                      isActive={tab.id === activeTabId}
                      isDragging={dragging}
                      isDropTarget={isTarget}
                      dropPosition={position}
                      onSelect={() => {
                        setActiveTab(paneId, tab.id)
                        setActivePane(paneId)
                      }}
                      onClose={() => {
                        useWorkspaceStore.getState().removeTab(tab.id)
                      }}
                      onDragStart={(e) => handleTabDragStart(tab.id, e)}
                      onDragOver={(e) => handleTabDragOver(tab.id, e)}
                      onDragLeave={handleTabDragLeave}
                      onDrop={(e) => handleTabDrop(tab.id, e)}
                    />
                  ) : tab.type === 'git-history' ? (
                    <GitHistoryTabInline
                      tab={tab as { type: 'git-history'; id: string; cwd: string }}
                      isActive={tab.id === activeTabId}
                      isDragging={dragging}
                      isDropTarget={isTarget}
                      dropPosition={position}
                      onSelect={() => {
                        setActiveTab(paneId, tab.id)
                        setActivePane(paneId)
                      }}
                      onClose={() => {
                        useWorkspaceStore.getState().removeTab(tab.id)
                      }}
                      onDragStart={(e) => handleTabDragStart(tab.id, e)}
                      onDragOver={(e) => handleTabDragOver(tab.id, e)}
                      onDragLeave={handleTabDragLeave}
                      onDrop={(e) => handleTabDrop(tab.id, e)}
                    />
                  ) : tab.type === 'agent-chat' ? (
                    <AgentChatTabInline
                      tab={tab as AgentChatTab}
                      isActive={tab.id === activeTabId}
                      isDragging={dragging}
                      isDropTarget={isTarget}
                      dropPosition={position}
                      onSelect={() => {
                        setActiveTab(paneId, tab.id)
                        setActivePane(paneId)
                      }}
                      onClose={() => {
                        useWorkspaceStore.getState().closeTab(paneId, tab.id)
                      }}
                      onDragStart={(e) => handleTabDragStart(tab.id, e)}
                      onDragOver={(e) => handleTabDragOver(tab.id, e)}
                      onDragLeave={handleTabDragLeave}
                      onDrop={(e) => handleTabDrop(tab.id, e)}
                    />
                  ) : (
                    <BrowserTabInline
                      tab={tab as { type: 'browser'; id: string; browserTabId: string }}
                      isActive={tab.id === activeTabId}
                      isDragging={dragging}
                      isDropTarget={isTarget}
                      dropPosition={position}
                      onSelect={() => {
                        setActiveTab(paneId, tab.id)
                        setActivePane(paneId)
                      }}
                      onClose={() => {
                        useBrowserSessionStore.getState().removeTab(tab.browserTabId)
                        useAnnotationStore.getState().clearAnnotationsForTab(tab.browserTabId)
                        useWorkspaceStore.getState().closeTab(paneId, tab.id)
                      }}
                      onDragStart={(e) => handleTabDragStart(tab.id, e)}
                      onDragOver={(e) => handleTabDragOver(tab.id, e)}
                      onDragLeave={handleTabDragLeave}
                      onDrop={(e) => handleTabDrop(tab.id, e)}
                    />
                  )}
                </div>
              )
            })}
          </div>
        </div>

        {hasOverflow && (
          <div className="pointer-events-none absolute right-0 top-0 h-full w-8 bg-gradient-to-l from-sidebar to-transparent" />
        )}
      </div>

      <div className="ml-auto flex h-full shrink-0 items-center border-l border-border/50">
        {leafCount > 1 && (
          <button
            type="button"
            onClick={() => togglePaneFullscreen(paneId)}
            className="flex h-full w-7 items-center justify-center text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-secondary hover:text-foreground"
            title={isFullscreenPane ? t('tabs.restorePane') : t('tabs.focusPane')}
            aria-label={isFullscreenPane ? t('tabs.restorePane') : t('tabs.focusPane')}
          >
            {isFullscreenPane ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
          </button>
        )}
        {onAddTerminal && (
          <div ref={terminalMenuRef} className="relative flex h-full items-center">
            <button
              type="button"
              onClick={() => setIsTerminalMenuOpen((open) => !open)}
              className="flex h-full w-7 items-center justify-center text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-secondary hover:text-foreground"
              title={t('tabs.terminalMenu')}
            >
              <TerminalIcon size={14} />
            </button>

            {isTerminalMenuOpen && (
              <div className="absolute top-full right-0 z-50 mt-px w-44 overflow-hidden border border-border/70 bg-popover shadow-md">
                <div className="bg-secondary/30 px-2.5 py-1 text-2xs font-medium text-muted-foreground">
                  {t('tabs.terminal')}
                </div>
                {loading ? (
                  <div className="space-y-1.5 px-2.5 py-1">
                    <Skeleton className="h-6 w-full" />
                    <Skeleton className="h-6 w-full" />
                  </div>
                ) : sortedShells && sortedShells.length > 0 ? (
                  <div className="py-1">
                    {sortedShells.map((shell) => (
                      <button
                        type="button"
                        key={shell.name}
                        onClick={() => handleSelectShell(shell)}
                        className={cn(
                          'flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-2xs leading-none hover:bg-secondary',
                          isPreferredShell(shell, defaultShell) && 'text-primary'
                        )}
                      >
                        <TerminalIcon size={11} />
                        <span className="truncate">{shell.displayName}</span>
                        {isPreferredShell(shell, defaultShell) && (
                          <span className="ml-auto text-3xs text-muted-foreground">
                            {t('tabs.default')}
                          </span>
                        )}
                      </button>
                    ))}
                  </div>
                ) : (
                  <div className="px-2.5 py-1.5 text-2xs text-muted-foreground">
                    {t('tabs.noShells')}
                  </div>
                )}
              </div>
            )}
          </div>
        )}

        {onAddBrowserTab && (
          <button
            type="button"
            onClick={onAddBrowserTab}
            className="flex h-full w-7 items-center justify-center text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-secondary hover:text-foreground"
            title={t('tabs.newBrowser')}
          >
            <Globe size={14} />
          </button>
        )}
      </div>
    </div>
  )
}
