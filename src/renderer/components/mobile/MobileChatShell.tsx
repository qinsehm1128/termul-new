import type { TerminalDisplayMode } from '@shared/types/ipc.types'
import {
  Camera,
  FolderGit2,
  FolderTree,
  GitBranch,
  History,
  Menu,
  MessageSquare,
  MessageSquarePlus,
  MoreHorizontal,
  Pencil,
  Plus,
  RotateCcw,
  Search,
  Settings,
  SlidersHorizontal,
  TerminalSquare,
  Trash2,
  X
} from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { useShallow } from 'zustand/shallow'
import { ConversationLifecycleActions } from '@/components/chat/ChatHistoryEntryRow'
import { ChatHistoryTab } from '@/components/chat/ChatHistoryTab'
import { ProjectSwitcherDrawer } from '@/components/chat/ProjectSwitcherDrawer'
import { CliSessionPanel } from '@/components/cli-sessions/CliSessionPanel'
import { ExecutionTargetPicker } from '@/components/conversation/ExecutionTargetPicker'
import { ListEmptyState, ListRow, ListRowMeta, pathBasename } from '@/components/lists'
import { SeMark } from '@/components/SeMark'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle
} from '@/components/ui/sheet'
import { CompanionTerminalGeometryContext } from '@/hooks/use-companion-terminal-geometry'
import { useOskViewport } from '@/hooks/use-osk-viewport'
import { logFrontendError } from '@/lib/log-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { cn } from '@/lib/utils'
import { useAcpStore } from '@/stores/acp-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useActiveProject, useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'
import { MobileFileExplorer } from './MobileFileExplorer'
import { MobileTerminalControls } from './MobileTerminalControls'

type MobileWorkspaceTab = 'chat' | 'terminal' | 'files'

interface MobileChatShellProps {
  children: React.ReactNode
  /** Opens the New Agent Chat launcher. */
  onNewChat: () => void
  /** Whether a new chat can be started. Conversation creation does not require a project. */
  canNewChat?: boolean
  /** Opens the command palette overlay (mounted in WorkspaceLayout appModals). */
  onOpenCommandPalette?: () => void
  /** Opens the Git Changes sheet (mounted in WorkspaceLayout mobile branch). */
  onOpenGitChanges?: () => void
  /** Opens a git history tab in the active pane (desktop entry mirrors this). */
  onOpenGitHistory?: () => void
  onNewTerminal?: () => void
  onCloseTerminal?: (terminalId: string, tabId: string) => void
  onTerminateTerminal?: (terminalId: string, tabId?: string) => void
  onRenameTerminal?: (terminalId: string, name: string) => void
  onRestartTerminal?: (terminalId: string) => void
}

function findWorkspaceTab(
  type: 'agent-chat' | 'terminal',
  root: Parameters<typeof getAllLeafPanes>[0],
  paneId: string
): { paneId: string; tabId: string } | null {
  const leaves = getAllLeafPanes(root)
  const activePane = leaves.find((leaf) => leaf.id === paneId) ?? leaves[0]
  const inActive = activePane?.tabs.find((tab) => tab.type === type)
  if (inActive && activePane) return { paneId: activePane.id, tabId: inActive.id }
  for (const leaf of leaves) {
    const tab = leaf.tabs.find((candidate) => candidate.type === type)
    if (tab) return { paneId: leaf.id, tabId: tab.id }
  }
  return null
}

function activateWorkspaceTab(paneId: string, tabId: string): void {
  const workspace = useWorkspaceStore.getState()
  if (workspace.activePaneId !== paneId) {
    requestAnimationFrame(() => {
      useWorkspaceStore.getState().setActiveTab(paneId, tabId)
    })
  } else {
    workspace.setActiveTab(paneId, tabId)
  }
}

/**
 * Compact mobile web chrome: slim header + three-tab workspace + overflow.
 * Desktop IDE chrome (ActivityRail, TitleBar, persistent sidebar, tab strip)
 * stays outside this component and must be gated by `useMobileWebShell`.
 */
export function MobileChatShell({
  children,
  onNewChat,
  canNewChat = true,
  onOpenCommandPalette,
  onOpenGitChanges,
  onOpenGitHistory,
  onNewTerminal,
  onCloseTerminal,
  onTerminateTerminal,
  onRenameTerminal,
  onRestartTerminal
}: MobileChatShellProps): React.JSX.Element {
  const { t } = useTranslation('mobile')
  const isWeb = !isTauriContext()
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [overflowOpen, setOverflowOpen] = useState(false)
  const [projectsOpen, setProjectsOpen] = useState(false)
  const [targetOpen, setTargetOpen] = useState(false)
  const [sessionsOpen, setSessionsOpen] = useState(false)
  const [surface, setSurface] = useState<MobileWorkspaceTab>('chat')
  const [preferredMode, setPreferredMode] = useState<TerminalDisplayMode>('phone')
  const osk = useOskViewport()
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renameValue, setRenameValue] = useState('')
  const navigate = useNavigate()
  const activeProject = useActiveProject()
  const projects = useProjectStore((state) => state.projects)
  const activeGroupName = useProjectStore((state) =>
    state.activeGroupId
      ? (state.groups.find((group) => group.id === state.activeGroupId)?.name ?? null)
      : null
  )

  // Active tab — return the stable Tab object reference held in the store
  // tree. Stable references compare with Object.is, so no `useShallow` is
  // needed. Returning a new object/array literal here would make every
  // getSnapshot() differ and trigger an infinite re-render loop
  // (React error #185 / Maximum update depth exceeded).
  const activeTab = useWorkspaceStore((s) => {
    const leaves = getAllLeafPanes(s.root)
    const pane = leaves.find((p) => p.id === s.activePaneId) ?? leaves[0]
    return pane?.tabs.find((tab) => tab.id === pane.activeTabId) ?? null
  })
  const activePaneId = useWorkspaceStore((s) => s.activePaneId)

  // Active terminal — subscribe to the terminal store directly (not via
  // getState() inside the workspace selector) so updates are observed and the
  // returned reference stays stable across unrelated workspace changes.
  const activeTerminalId = activeTab?.type === 'terminal' ? activeTab.terminalId : undefined
  const activeTerminal = useTerminalStore((s) =>
    activeTerminalId ? s.terminals.find((terminal) => terminal.id === activeTerminalId) : undefined
  )
  const allTerminals = useTerminalStore(useShallow((state) => state.terminals))

  // Terminal tabs across ALL leaf panes. Derive via useMemo from the stable
  // `root` reference so the wrapper objects are only rebuilt when the tree
  // actually changes — never on every render (which would re-trigger the loop).
  const workspaceRoot = useWorkspaceStore((s) => s.root)
  const terminalTabs = useMemo(() => {
    const leaves = getAllLeafPanes(workspaceRoot)
    return leaves.flatMap((leaf) =>
      (leaf.tabs ?? [])
        .filter((tab) => tab.type === 'terminal')
        .map((tab) => ({ tab, paneId: leaf.id }))
    )
  }, [workspaceRoot])

  const activeConversationId = useConversationStore((state) => state.activeConversationId)
  const activeConversation = useConversationStore((state) =>
    activeConversationId ? state.summariesById[activeConversationId] : undefined
  )
  const [mobileTarget, setMobileTarget] = useState(
    activeConversation?.executionTarget ?? {
      kind: 'workspace' as const
    }
  )
  const [mobileAttachment, setMobileAttachment] = useState(
    activeConversation?.projectAttachment ?? null
  )

  useEffect(() => {
    if (!activeConversation) return
    setMobileTarget(activeConversation.executionTarget)
    setMobileAttachment(activeConversation.projectAttachment)
  }, [activeConversation])

  const activeSessionId = useAcpStore((state) => {
    if (activeTab?.type !== 'agent-chat') return null
    if (activeTab.sessionId) return activeTab.sessionId
    const conversationId = activeTab.conversationId ?? activeConversationId
    if (!conversationId) return null
    return (
      Object.values(state.sessions).find((session) => session.conversationId === conversationId)
        ?.id ??
      state.sessionIndex.find((entry) => entry.conversationId === conversationId)?.id ??
      null
    )
  })

  const sessionTitle = useAcpStore((s) => {
    if (!activeSessionId) return null
    const live = s.sessions[activeSessionId]?.title
    if (live) return live
    return s.sessionIndex.find((e) => e.id === activeSessionId)?.title ?? null
  })
  const conversationTerminals = useMemo(
    () =>
      activeConversationId
        ? allTerminals.filter((terminal) => terminal.conversationId === activeConversationId)
        : terminalTabs
            .map(({ tab }) => allTerminals.find((terminal) => terminal.id === tab.terminalId))
            .filter((terminal): terminal is NonNullable<typeof terminal> => Boolean(terminal)),
    [activeConversationId, allTerminals, terminalTabs]
  )
  const hasLiveTerminal = allTerminals.some((terminal) => Boolean(terminal.ptyId))

  const headerTitle = useMemo(() => {
    if (surface === 'files') return t('chatShell.tabFiles')
    if (surface === 'terminal' && !activeTerminal?.name) return t('chatShell.tabTerminal')
    if (activeTerminal?.name && (surface === 'terminal' || activeTab?.type === 'terminal')) {
      return activeTerminal.name
    }
    if (sessionTitle) return sessionTitle
    if (activeGroupName) return activeGroupName
    if (activeProject?.name) return activeProject.name
    return 'Termul'
  }, [
    surface,
    activeTerminal?.name,
    activeTab?.type,
    sessionTitle,
    activeGroupName,
    activeProject?.name,
    t
  ])

  const headerSubtitle = useMemo(() => {
    if (surface === 'files') return activeProject?.path ?? null
    if (surface === 'terminal' && activeTerminal?.cwd) return activeTerminal.cwd
    if (activeConversation?.workspaceCwd) return activeConversation.workspaceCwd
    return activeProject?.path ?? null
  }, [surface, activeProject?.path, activeTerminal?.cwd, activeConversation?.workspaceCwd])

  const closeDrawer = (): void => setDrawerOpen(false)

  const selectSurface = (next: MobileWorkspaceTab): void => {
    if (next === surface) return
    void logFrontendError({
      level: 'warn',
      source: 'mobile-shell.tab',
      message: `from=${surface} to=${next}`
    })
    setSurface(next)

    if (next === 'chat' && activeTab?.type === 'terminal') {
      const chat = findWorkspaceTab('agent-chat', workspaceRoot, activePaneId)
      if (chat) activateWorkspaceTab(chat.paneId, chat.tabId)
      return
    }

    if (next === 'terminal' && activeTab?.type !== 'terminal') {
      const terminal = findWorkspaceTab('terminal', workspaceRoot, activePaneId)
      if (terminal) activateWorkspaceTab(terminal.paneId, terminal.tabId)
    }
  }

  const selectTerminal = (paneId: string, tabId: string): void => {
    setSurface('terminal')
    activateWorkspaceTab(paneId, tabId)
    closeDrawer()
  }

  const startRename = (terminalId: string, currentName: string): void => {
    setRenamingId(terminalId)
    setRenameValue(currentName)
  }

  const confirmRename = (): void => {
    if (renamingId && renameValue.trim() && onRenameTerminal) {
      onRenameTerminal(renamingId, renameValue.trim())
    }
    setRenamingId(null)
    setRenameValue('')
  }

  const closeOverflow = (): void => setOverflowOpen(false)

  const showPaneChildren = surface === 'chat' || (surface === 'terminal' && terminalTabs.length > 0)
  const showTerminalEmpty = surface === 'terminal' && terminalTabs.length === 0
  const showFiles = surface === 'files'
  const companionGeometry = useMemo(
    () => ({
      surfaceActive: surface === 'terminal',
      preferredMode,
      keyboardOpen: osk.isOskOpen,
      setPreferredMode
    }),
    [surface, preferredMode, osk.isOskOpen]
  )

  return (
    <CompanionTerminalGeometryContext.Provider value={companionGeometry}>
      <div className="flex h-full min-h-0 flex-col bg-background" data-mobile-chat-shell="">
        <header className="flex min-h-11 shrink-0 items-center gap-1 border-b border-border/70 bg-sidebar px-1.5 py-1">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-11 shrink-0"
            aria-label={t('chatShell.openMenu')}
            aria-expanded={drawerOpen}
            aria-controls={drawerOpen ? 'mobile-chat-drawer' : undefined}
            onClick={() => setDrawerOpen(true)}
          >
            <Menu size={20} />
          </Button>

          <div className="min-w-0 flex-1 text-center">
            <h1 className="truncate text-sm font-medium text-foreground">{headerTitle}</h1>
            {headerSubtitle && headerSubtitle !== headerTitle ? (
              <p className="truncate font-mono text-[11px] text-muted-foreground">
                {headerSubtitle}
              </p>
            ) : null}
          </div>

          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-11 shrink-0"
            aria-label={t('chatShell.more')}
            aria-expanded={overflowOpen}
            onClick={() => setOverflowOpen(true)}
          >
            <MoreHorizontal size={20} />
          </Button>
        </header>

        <div
          className={cn('min-h-0 flex-1 overflow-hidden', !showPaneChildren && 'hidden')}
          data-mobile-workspace-surface="pane"
        >
          {children}
        </div>

        {showTerminalEmpty ? (
          <div
            className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-6 text-center"
            data-mobile-workspace-surface="terminal-empty"
          >
            <p className="text-sm text-muted-foreground">{t('chatShell.noOpenTerminals')}</p>
            <p className="text-xs text-muted-foreground">{t('chatShell.noOpenTerminalsHint')}</p>
            <Button type="button" className="h-11 min-h-11 px-4" onClick={() => onNewTerminal?.()}>
              {t('chatShell.newTerminal')}
            </Button>
          </div>
        ) : null}

        {isWeb ? (
          <div
            className={cn('min-h-0 flex-1 overflow-hidden', !showFiles && 'hidden')}
            data-mobile-workspace-surface="files"
          >
            <MobileFileExplorer
              variant="page"
              open={surface === 'files'}
              onFileOpened={() => selectSurface('chat')}
            />
          </div>
        ) : showFiles ? (
          <div
            className="flex min-h-0 flex-1 items-center justify-center px-6 text-center"
            data-mobile-workspace-surface="files"
          >
            <p className="text-sm text-muted-foreground">
              {activeProject?.path ? t('files.useDesktopExplorer') : t('files.noProject')}
            </p>
          </div>
        ) : null}

        {surface === 'terminal' && activeTerminal?.ptyId ? (
          <MobileTerminalControls terminalId={activeTerminal.ptyId} />
        ) : null}

        <div
          role="tablist"
          aria-label={t('chatShell.workspaceTabs')}
          data-od-id="workspace-tabs"
          data-mobile-workspace-tabs=""
          className="flex shrink-0 border-t border-border/70 bg-card/95 pb-[max(0.25rem,env(safe-area-inset-bottom))] backdrop-blur"
        >
          {(
            [
              ['chat', MessageSquare, t('chatShell.tabChat')],
              ['terminal', TerminalSquare, t('chatShell.tabTerminal')],
              ['files', FolderTree, t('chatShell.tabFiles')]
            ] as const
          ).map(([id, Icon, label]) => {
            const selected = surface === id
            return (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={selected}
                aria-label={label}
                className={cn(
                  'relative flex min-h-11 flex-1 flex-col items-center justify-center gap-0.5 text-[11px]',
                  selected ? 'text-primary' : 'text-muted-foreground'
                )}
                onClick={() => selectSurface(id)}
              >
                <Icon size={18} />
                {id === 'terminal' && hasLiveTerminal ? (
                  <span
                    className="absolute right-1/2 top-1.5 size-1.5 translate-x-2.5 rounded-full bg-connection"
                    aria-hidden
                  />
                ) : null}
                <span>{label}</span>
              </button>
            )
          })}
        </div>

        <Sheet open={overflowOpen} onOpenChange={setOverflowOpen}>
          <SheetContent
            side="bottom"
            className="flex max-h-[85dvh] flex-col gap-0 rounded-t-xl p-2"
          >
            <SheetHeader className="px-3 py-2 text-left">
              <SheetTitle>{t('chatShell.more')}</SheetTitle>
              <SheetDescription className="sr-only">{t('chatShell.more')}</SheetDescription>
            </SheetHeader>
            <div className="overflow-y-auto pb-2">
              <OverflowItem
                label={t('chatShell.newChat')}
                disabled={!canNewChat}
                onSelect={() => {
                  closeOverflow()
                  onNewChat()
                }}
              >
                <MessageSquarePlus size={16} />
              </OverflowItem>
              {activeConversationId && activeConversation ? (
                <>
                  <OverflowItem
                    label={t('chatShell.executionTarget')}
                    onSelect={() => {
                      closeOverflow()
                      setTargetOpen(true)
                    }}
                  >
                    <SlidersHorizontal size={16} />
                  </OverflowItem>
                  <div className="flex h-11 min-h-11 items-center px-1">
                    <ConversationLifecycleActions
                      conversationId={activeConversationId}
                      title={sessionTitle ?? t('chatShell.chats')}
                      className="size-11 w-full justify-start opacity-100 after:inset-0"
                    />
                  </div>
                </>
              ) : null}
              {surface === 'terminal' && activeTab?.type === 'terminal' && onRestartTerminal ? (
                <OverflowItem
                  label={t('chatShell.restartTerminal')}
                  onSelect={() => {
                    closeOverflow()
                    onRestartTerminal(activeTab.terminalId)
                  }}
                >
                  <RotateCcw size={16} />
                </OverflowItem>
              ) : null}
              <OverflowItem
                label={t('chatShell.terminalBoard')}
                onSelect={() => {
                  closeOverflow()
                  navigate('/terminals')
                }}
              >
                <TerminalSquare size={16} />
              </OverflowItem>
              <OverflowItem
                label={t('chatShell.settings')}
                onSelect={() => {
                  closeOverflow()
                  navigate('/preferences')
                }}
              >
                <Settings size={16} />
              </OverflowItem>
              <OverflowItem
                label={t('chatShell.snapshots')}
                onSelect={() => {
                  closeOverflow()
                  navigate('/snapshots')
                }}
              >
                <Camera size={16} />
              </OverflowItem>
              {isWeb ? (
                <>
                  <OverflowItem
                    label={t('chatShell.switchProject')}
                    onSelect={() => {
                      closeOverflow()
                      setProjectsOpen(true)
                    }}
                  >
                    <FolderGit2 size={16} />
                  </OverflowItem>
                  <OverflowItem
                    label={t('chatShell.cliSessions')}
                    onSelect={() => {
                      closeOverflow()
                      setSessionsOpen(true)
                    }}
                  >
                    <History size={16} />
                  </OverflowItem>
                  {onOpenCommandPalette ? (
                    <OverflowItem
                      label={t('chatShell.commandPalette')}
                      onSelect={() => {
                        closeOverflow()
                        onOpenCommandPalette()
                      }}
                    >
                      <Search size={16} />
                    </OverflowItem>
                  ) : null}
                  {onOpenGitChanges ? (
                    <OverflowItem
                      label={t('chatShell.gitChanges')}
                      disabled={!activeProject?.path}
                      onSelect={() => {
                        closeOverflow()
                        onOpenGitChanges()
                      }}
                    >
                      <GitBranch size={16} />
                    </OverflowItem>
                  ) : null}
                  {onOpenGitHistory ? (
                    <OverflowItem
                      label={t('chatShell.gitHistory')}
                      disabled={!activeProject?.path}
                      onSelect={() => {
                        closeOverflow()
                        onOpenGitHistory()
                      }}
                    >
                      <History size={16} />
                    </OverflowItem>
                  ) : null}
                </>
              ) : null}
            </div>
          </SheetContent>
        </Sheet>

        <Sheet open={drawerOpen} onOpenChange={setDrawerOpen}>
          <SheetContent
            side="left"
            id="mobile-chat-drawer"
            className="flex w-[min(100vw-3rem,20rem)] flex-col gap-0 p-0 sm:max-w-sm"
          >
            <SheetHeader className="space-y-0 border-b border-border/60 px-4 py-3 text-left">
              <div className="flex items-center gap-2 pr-8">
                <SeMark size={20} />
                <SheetTitle className="text-base">{t('chatShell.chats')}</SheetTitle>
              </div>
              <SheetDescription className="sr-only">
                {t('chatShell.chatDescription')}
              </SheetDescription>
            </SheetHeader>

            <div className="flex shrink-0 border-b border-border/60 p-2">
              <Button
                type="button"
                variant="secondary"
                className="h-11 min-h-11 flex-1 justify-start gap-2"
                disabled={!canNewChat}
                onClick={() => {
                  closeDrawer()
                  onNewChat()
                }}
              >
                <MessageSquarePlus size={16} />
                {t('chatShell.newChat')}
              </Button>
            </div>

            {isWeb ? (
              <div
                className="flex shrink-0 gap-1 border-b border-border/60 p-1"
                role="tablist"
                aria-label={t('chatShell.deskSessions')}
              >
                <button
                  type="button"
                  role="tab"
                  aria-selected
                  className="flex h-11 min-h-11 flex-1 items-center justify-center rounded-md bg-secondary text-xs font-medium text-foreground"
                >
                  {t('chatShell.deskSessions')}
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-label={t('chatShell.switchProject')}
                  className="flex h-11 min-h-11 flex-1 items-center justify-center rounded-md text-xs font-medium text-muted-foreground"
                  onClick={() => {
                    closeDrawer()
                    setProjectsOpen(true)
                  }}
                >
                  {t('chatShell.deskProjects')}
                </button>
              </div>
            ) : null}

            <div className="border-b border-border/60 p-2">
              <div className="mb-1 flex items-center justify-between px-2 text-xs font-medium text-muted-foreground">
                <span>{t('chatShell.terminals')}</span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-11"
                  aria-label={t('chatShell.newTerminal')}
                  onClick={() => {
                    closeDrawer()
                    onNewTerminal?.()
                  }}
                >
                  <Plus size={16} />
                </Button>
              </div>
              {conversationTerminals.length === 0 ? (
                <ListEmptyState message={t('chatShell.noOpenTerminals')} />
              ) : (
                conversationTerminals.map((terminal) => {
                  const tabEntry = terminalTabs.find(
                    (entry) => entry.tab.terminalId === terminal.id
                  )
                  const isActive = tabEntry?.tab.id === activeTab?.id
                  const isRenaming = renamingId === terminal.id
                  const isHidden = terminal.viewState !== 'visible' || !tabEntry
                  const folder = pathBasename(terminal.cwd)
                  return (
                    <ListRow
                      key={terminal.id}
                      density="compact"
                      active={isActive}
                      buttonClassName="h-11"
                      title={
                        <span className="flex min-w-0 items-center gap-2">
                          <TerminalSquare size={16} />
                          <span className="truncate">{terminal.name}</span>
                        </span>
                      }
                      titleAttr={terminal.name}
                      preview={isHidden ? t('chatShell.reopenTerminal') : folder || undefined}
                      meta={
                        <ListRowMeta
                          items={[
                            isHidden ? t('chatShell.hidden') : t('chatShell.running'),
                            terminal.agentName ?? terminal.shell
                          ]}
                        />
                      }
                      trailing={
                        <>
                          {isRenaming ? (
                            <input
                              type="text"
                              value={renameValue}
                              onChange={(event) => setRenameValue(event.target.value)}
                              onBlur={confirmRename}
                              onKeyDown={(event) => {
                                if (event.key === 'Enter') confirmRename()
                                if (event.key === 'Escape') setRenamingId(null)
                              }}
                              className="h-11 w-24 rounded border border-border bg-background px-2 text-xs"
                              autoFocus
                            />
                          ) : (
                            onRenameTerminal && (
                              <Button
                                type="button"
                                variant="ghost"
                                size="icon"
                                className="size-11 shrink-0"
                                aria-label={t('chatShell.renameTerminal')}
                                onClick={() => startRename(terminal.id, terminal.name)}
                              >
                                <Pencil size={14} />
                              </Button>
                            )
                          )}
                          {!isHidden && tabEntry && onCloseTerminal && (
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon"
                              className="size-11 shrink-0"
                              aria-label={t('chatShell.closeTerminalView')}
                              onClick={() => onCloseTerminal(terminal.id, tabEntry.tab.id)}
                            >
                              <X size={14} />
                            </Button>
                          )}
                          {onTerminateTerminal && (
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon"
                              className="size-11 shrink-0 text-destructive hover:text-destructive"
                              aria-label={t('chatShell.terminateTerminal')}
                              onClick={() => onTerminateTerminal(terminal.id, tabEntry?.tab.id)}
                            >
                              <Trash2 size={14} />
                            </Button>
                          )}
                        </>
                      }
                      onClick={() => {
                        if (tabEntry) selectTerminal(tabEntry.paneId, tabEntry.tab.id)
                        else {
                          useWorkspaceStore.getState().reopenTerminalView(terminal.id)
                          setSurface('terminal')
                          closeDrawer()
                        }
                      }}
                    />
                  )
                })
              )}
            </div>

            <div className="min-h-0 flex-1 overflow-hidden">
              <ChatHistoryTab
                onSessionOpened={() => {
                  setSurface('chat')
                  closeDrawer()
                }}
              />
            </div>
          </SheetContent>
        </Sheet>

        <Sheet open={targetOpen} onOpenChange={setTargetOpen}>
          <SheetContent side="bottom" className="max-h-[85dvh] overflow-y-auto">
            <SheetHeader className="text-left">
              <SheetTitle>{t('chatShell.executionTarget')}</SheetTitle>
              <SheetDescription>{t('chatShell.executionTargetDescription')}</SheetDescription>
            </SheetHeader>
            {activeConversation ? (
              <div className="mt-4">
                <ExecutionTargetPicker
                  projects={projects}
                  value={mobileTarget}
                  attachment={mobileAttachment}
                  conversation={activeConversation}
                  workspaceCwd={activeConversation.workspaceCwd}
                  onChange={setMobileTarget}
                  onAttachmentChange={setMobileAttachment}
                />
              </div>
            ) : null}
          </SheetContent>
        </Sheet>

        {isWeb && <ProjectSwitcherDrawer open={projectsOpen} onOpenChange={setProjectsOpen} />}

        {isWeb && (
          <Sheet open={sessionsOpen} onOpenChange={setSessionsOpen}>
            <SheetContent
              side="right"
              className="flex w-[min(100vw-2rem,22rem)] flex-col gap-0 p-0 sm:max-w-sm"
            >
              <SheetHeader className="sr-only">
                <SheetTitle>{t('chatShell.cliSessions')}</SheetTitle>
                <SheetDescription>{t('chatShell.cliSessionsDescription')}</SheetDescription>
              </SheetHeader>
              <CliSessionPanel forceVisible className="h-full w-full rounded-none" />
            </SheetContent>
          </Sheet>
        )}
      </div>
    </CompanionTerminalGeometryContext.Provider>
  )
}

function OverflowItem({
  label,
  disabled,
  onSelect,
  children
}: {
  label: string
  disabled?: boolean
  onSelect: () => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <button
      type="button"
      aria-label={label}
      disabled={disabled}
      className="flex h-11 min-h-11 w-full items-center gap-3 rounded-md px-3 text-left text-sm hover:bg-accent disabled:pointer-events-none disabled:opacity-45"
      onClick={onSelect}
    >
      {children}
      <span className="min-w-0 truncate">{label}</span>
    </button>
  )
}
