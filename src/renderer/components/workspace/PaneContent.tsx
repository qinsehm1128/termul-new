import type { ShellInfo } from '@shared/types/ipc.types'
import { RefreshCcw, Unplug, X } from 'lucide-react'
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation } from 'react-router-dom'
// Import useShallow for selective re-rendering
import { useShallow } from 'zustand/shallow'
import { AgentIcon } from '@/components/agents/AgentIcon'
import { AgentLauncher } from '@/components/agents/AgentLauncher'
import { Skeleton } from '@/components/ui/skeleton'
import { useMobileWebShell } from '@/hooks/use-mobile-web-shell'
import { usePaneDnd } from '@/hooks/use-pane-dnd'
import { resolveConversationSessionId } from '@/lib/conversation-binding'
import { logFrontendError } from '@/lib/log-api'
import { cn } from '@/lib/utils'
import { useAcpStore } from '@/stores/acp-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useTerminalActions, useTerminalStore } from '@/stores/terminal-store'
import type { AgentChatTab, WorkspaceTab } from '@/stores/workspace-store'
import { getAllLeafPanes, retireTerminalRecord, useWorkspaceStore } from '@/stores/workspace-store'
import type { LeafNode } from '@/types/workspace.types'
import { DropZoneOverlay } from './DropZoneOverlay'
import { WorkspaceTabBar } from './WorkspaceTabBar'

/** Inactive tabs stay mounted but must not intercept clicks on the active tab beneath. */
const INACTIVE_TAB_PANE_CLASS = 'w-full h-full absolute inset-0 invisible pointer-events-none'

const AgentChatPanel = lazy(() =>
  import('@/components/chat/AgentChatPanel').then((m) => ({ default: m.AgentChatPanel }))
)
const BrowserPanel = lazy(() =>
  import('@/components/browser/BrowserPanel').then((m) => ({ default: m.BrowserPanel }))
)
const ConnectedTerminal = lazy(() =>
  import('@/components/terminal/ConnectedTerminal').then((m) => ({ default: m.ConnectedTerminal }))
)
const EditorPanel = lazy(() =>
  import('@/components/editor/EditorPanel').then((m) => ({ default: m.EditorPanel }))
)
const GitHistoryPanel = lazy(() =>
  import('@/components/git/GitHistoryPanel').then((m) => ({ default: m.GitHistoryPanel }))
)
const GitPanel = lazy(() =>
  import('@/components/git/GitPanel').then((m) => ({ default: m.GitPanel }))
)

/** Lightweight Suspense fallback for lazy-loaded panes (reuses Skeleton). */
function PaneSkeleton(): React.JSX.Element {
  return <Skeleton className="h-full w-full" />
}

function ConversationAgentChatPanel({
  tab,
  paneId,
  isVisible
}: {
  tab: AgentChatTab
  paneId: string
  isVisible: boolean
}): React.JSX.Element {
  const sessionId = useAcpStore((state) => {
    if (tab.sessionId) return tab.sessionId
    if (!tab.conversationId) return null
    return resolveConversationSessionId(state, tab.conversationId)
  })
  const opening = useConversationStore((state) =>
    tab.conversationId ? state.openingById[tab.conversationId] === true : false
  )

  if (!sessionId) {
    if (opening) return <PaneSkeleton />
    // This tab already owns a Conversation; the launcher is its restart
    // surface, not a new-chat composer. Name the target explicitly — the
    // launcher deliberately no longer infers one from the sidebar selection.
    return <AgentLauncher paneId={paneId} continueConversationId={tab.conversationId} />
  }
  return <AgentChatPanel sessionId={sessionId} paneId={paneId} isVisible={isVisible} />
}

interface PaneContentProps {
  pane: LeafNode
  onAddTerminal?: (paneId: string, shell?: ShellInfo) => void
  onAddBrowserTab?: (paneId: string) => void
  onCloseTerminal?: (id: string, tabId: string) => void
  onRenameTerminal?: (id: string, name: string) => void
  onCloseEditorTab?: (filePath: string) => void
  closingTerminalIds?: string[]
  defaultShell?: string
}

export function PaneContent({
  pane,
  onAddTerminal,
  onAddBrowserTab,
  onCloseTerminal,
  onRenameTerminal,
  onCloseEditorTab,
  closingTerminalIds = [],
  defaultShell
}: PaneContentProps): React.JSX.Element {
  const { t } = useTranslation(['workspace', 'terminal'])
  const location = useLocation()
  const isConversationWorkspace = location.pathname.startsWith('/c/')
  // CRITICAL FIX: Get terminal IDs from this pane's tabs
  const terminalIdsInPane = useMemo(
    () => new Set(pane.tabs.filter((t) => t.type === 'terminal').map((t) => t.terminalId)),
    [pane.tabs]
  )

  // Subscribe to terminals referenced by this pane's tabs. Do not filter by
  // active projectId: conversation-scoped terminals must still render, and
  // stale project-id mismatches previously produced a blank pane.
  const terminalsInPane = useTerminalStore(
    useShallow((state) => state.terminals.filter((t) => terminalIdsInPane.has(t.id)))
  )

  // FIX: Batch workspace store subscriptions with useShallow to prevent cascading re-renders
  const { activePaneId, fullscreenPaneId, agentLauncherPaneId, setActivePane } = useWorkspaceStore(
    useShallow((state) => ({
      activePaneId: state.activePaneId,
      fullscreenPaneId: state.fullscreenPaneId,
      agentLauncherPaneId: state.agentLauncherPaneId,
      setActivePane: state.setActivePane
    }))
  )

  const hasMultiplePanes = useWorkspaceStore((state) => getAllLeafPanes(state.root).length > 1)

  const { setTerminalPtyId } = useTerminalActions()
  const { isDragging, previewTarget } = usePaneDnd()
  const isMobileWebShell = useMobileWebShell()

  const isFullscreenPane = fullscreenPaneId === pane.id
  const isActivePane = activePaneId === pane.id
  const activeTab = pane.tabs.find((t) => t.id === pane.activeTabId)
  const activeTerminalIdInPane = activeTab?.type === 'terminal' ? activeTab.terminalId : null
  const panePreviewPosition =
    previewTarget?.paneId === pane.id && !isFullscreenPane ? previewTarget.position : null

  // Agent loading: show pulsing icon for a minimum duration after the terminal
  // is first seen. The xterm renderer attaches almost instantly (same frame),
  // so rendererAttachmentCount alone isn't enough for a visible loading state.
  const [agentLoadingIds, setAgentLoadingIds] = useState<Set<string>>(new Set())
  const [retryingTerminalIds, setRetryingTerminalIds] = useState<Set<string>>(new Set())
  const agentLoadingTimers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map())
  const prevAgentTerminalIdsRef = useRef<string[]>([])
  const AGENT_LOADING_MS = 1500

  // Detect newly appeared agent terminals and start loading timers.
  // This runs after every render where terminalsInPane changes (useShallow).
  useEffect(() => {
    const currentIds = terminalsInPane
      .filter((t) => t.kind === 'agent' && t.agentId && t.ptyId)
      .map((t) => t.id)
    const prevIds = prevAgentTerminalIdsRef.current
    prevAgentTerminalIdsRef.current = currentIds

    for (const id of currentIds) {
      if (!prevIds.includes(id) && !agentLoadingTimers.current.has(id)) {
        // New agent terminal — add to loading set with a minimum duration.
        setAgentLoadingIds((prev) => new Set(prev).add(id))
        agentLoadingTimers.current.set(
          id,
          setTimeout(() => {
            setAgentLoadingIds((prev) => {
              const next = new Set(prev)
              next.delete(id)
              return next
            })
            agentLoadingTimers.current.delete(id)
          }, AGENT_LOADING_MS)
        )
      }
    }
  }, [terminalsInPane])

  const handleFocus = useCallback(() => {
    if (!isActivePane) {
      setActivePane(pane.id)
    }
    // Clicking into the pane is an explicit acknowledgment of its visible terminal:
    // clear the finished-terminal highlight. This is the ONLY clear path — we do not
    // auto-clear on tab-switch or remount, so a flagged background tab keeps its border
    // until the user actually looks at it.
    if (activeTerminalIdInPane) {
      const store = useTerminalStore.getState()
      const term = store.terminals.find((t) => t.id === activeTerminalIdInPane)
      if (term?.needsAttention) {
        store.setTerminalNeedsAttention(activeTerminalIdInPane, false)
      }
    }
  }, [isActivePane, setActivePane, pane.id, activeTerminalIdInPane])

  // Keyboard parity for the mouse acknowledgment above: a keystroke directed at this
  // pane's visible terminal is an explicit "I'm looking at it" signal, so clear the
  // highlight. Capture phase + a real key event means this never fires on a passive
  // tab-switch (which only auto-focuses the terminal, no keypress), avoiding the
  // flash-and-vanish that auto-clear-on-visibility caused.
  const handleKeyDownCapture = useCallback(() => {
    if (!activeTerminalIdInPane) return
    const store = useTerminalStore.getState()
    const term = store.terminals.find((t) => t.id === activeTerminalIdInPane)
    if (term?.needsAttention) {
      store.setTerminalNeedsAttention(activeTerminalIdInPane, false)
    }
  }, [activeTerminalIdInPane])

  const retryDisconnectedTerminal = useCallback(async (terminalId: string): Promise<void> => {
    setRetryingTerminalIds((current) => new Set(current).add(terminalId))
    try {
      const result = await useTerminalStore.getState().resumeTerminalResource(terminalId)
      if (!result.success) {
        void logFrontendError({
          level: 'warn',
          source: 'pane-content.terminal-resume',
          message: `code=${result.code} terminalRecordId=${terminalId}`
        })
        // Retrying a terminal the host reports as gone can never succeed. Retire
        // it instead of leaving the user on a placeholder whose only action is
        // the button they just pressed. Unlike the cold-load path there is no
        // topology rebuild here, so the tab has to go with the record — the
        // missing-terminal branch below would otherwise render the same
        // "disconnected" copy minus the retry button.
        if (result.code === 'TERMINAL_GONE') retireTerminalRecord(terminalId)
      }
    } finally {
      setRetryingTerminalIds((current) => {
        const next = new Set(current)
        next.delete(terminalId)
        return next
      })
    }
  }, [])

  const previewSpaceClass =
    panePreviewPosition === 'left'
      ? 'pl-6'
      : panePreviewPosition === 'right'
        ? 'pr-6'
        : panePreviewPosition === 'top'
          ? 'pt-6'
          : panePreviewPosition === 'bottom'
            ? 'pb-6'
            : ''

  const previewTranslateClass =
    panePreviewPosition === 'left'
      ? 'translate-x-2'
      : panePreviewPosition === 'right'
        ? '-translate-x-2'
        : panePreviewPosition === 'top'
          ? 'translate-y-2'
          : panePreviewPosition === 'bottom'
            ? '-translate-y-2'
            : ''

  const handleAddTerminalForPane = useMemo(
    () => (onAddTerminal ? (shell?: ShellInfo) => onAddTerminal(pane.id, shell) : undefined),
    [onAddTerminal, pane.id]
  )
  const handleAddBrowserTabForPane = useMemo(
    () => (onAddBrowserTab ? () => onAddBrowserTab(pane.id) : undefined),
    [onAddBrowserTab, pane.id]
  )

  return (
    <div
      className={cn(
        'flex flex-col h-full relative',
        isActivePane &&
          hasMultiplePanes &&
          !isFullscreenPane &&
          'shadow-[inset_0_0_0_1px_hsl(var(--primary)/0.34)]',
        isFullscreenPane && 'overflow-hidden shadow-[inset_0_0_0_1px_hsl(var(--primary)/0.34)]'
      )}
      onMouseDown={handleFocus}
      onKeyDownCapture={handleKeyDownCapture}
    >
      {!isMobileWebShell && (
        <WorkspaceTabBar
          paneId={pane.id}
          tabs={pane.tabs}
          activeTabId={pane.activeTabId}
          closingTerminalIds={closingTerminalIds}
          onAddTerminal={handleAddTerminalForPane}
          onAddBrowserTab={handleAddBrowserTabForPane}
          onCloseTerminal={onCloseTerminal}
          onRenameTerminal={onRenameTerminal}
          onCloseEditorTab={onCloseEditorTab}
          defaultShell={defaultShell}
        />
      )}

      <div className="flex-1 overflow-hidden bg-terminal-bg relative h-full">
        <div
          className={cn(
            'w-full h-full relative transition-all duration-150 ease-out',
            previewSpaceClass
          )}
        >
          {(panePreviewPosition === 'left' || panePreviewPosition === 'right') && (
            <div
              className={cn(
                'pointer-events-none absolute top-0 bottom-0 w-4 border-primary/35 bg-primary/8',
                panePreviewPosition === 'left' ? 'left-0 border-r' : 'right-0 border-l'
              )}
            />
          )}
          {(panePreviewPosition === 'top' || panePreviewPosition === 'bottom') && (
            <div
              className={cn(
                'pointer-events-none absolute left-0 right-0 h-4 border-primary/35 bg-primary/8',
                panePreviewPosition === 'top' ? 'top-0 border-b' : 'bottom-0 border-t'
              )}
            />
          )}

          <div
            className={cn(
              'w-full h-full relative transition-transform duration-150 ease-out',
              previewTranslateClass
            )}
          >
            {pane.tabs
              .filter((t): t is WorkspaceTab & { type: 'terminal' } => t.type === 'terminal')
              .map((tab) => {
                const terminal = terminalsInPane.find((t) => t.id === tab.terminalId)
                const isVisible = activeTab?.id === tab.id
                if (!terminal) {
                  if (!isVisible) {
                    return null
                  }
                  const missingTitleId = `terminal-missing-${tab.terminalId}`
                  return (
                    <section
                      key={tab.id}
                      className="flex h-full w-full items-center justify-center p-6"
                      aria-labelledby={missingTitleId}
                      data-terminal-health="missing"
                    >
                      <div className="flex w-full max-w-md flex-col items-center gap-3 border border-border/50 bg-card/40 px-5 py-6 text-center">
                        <span className="flex h-10 w-10 items-center justify-center bg-muted text-muted-foreground">
                          <Unplug className="h-5 w-5" aria-hidden="true" />
                        </span>
                        <div className="space-y-2">
                          <h2 id={missingTitleId} className="text-base font-semibold">
                            {t('resume.disconnectedTitle', { ns: 'terminal' })}
                          </h2>
                          <p
                            className="text-sm leading-relaxed text-muted-foreground"
                            role="status"
                            aria-live="polite"
                          >
                            {t('resume.disconnectedDescription', { ns: 'terminal' })}
                          </p>
                        </div>
                      </div>
                    </section>
                  )
                }
                if (terminal.healthStatus === 'disconnected') {
                  const isRetrying = retryingTerminalIds.has(terminal.id)
                  const disconnectedTitleId = `terminal-disconnected-${terminal.id}`
                  return (
                    <section
                      key={tab.id}
                      className={cn(
                        isVisible ? 'w-full h-full' : INACTIVE_TAB_PANE_CLASS,
                        'flex items-center justify-center p-6'
                      )}
                      aria-labelledby={disconnectedTitleId}
                      data-terminal-health="disconnected"
                    >
                      <div className="flex w-full max-w-md flex-col items-center gap-3 border border-border/50 bg-card/40 px-5 py-6 text-center">
                        <span className="flex h-10 w-10 items-center justify-center bg-muted text-muted-foreground">
                          <Unplug className="h-5 w-5" aria-hidden="true" />
                        </span>
                        <div className="space-y-2">
                          <h2 id={disconnectedTitleId} className="text-base font-semibold">
                            {t('resume.disconnectedTitle', { ns: 'terminal' })}
                          </h2>
                          <p
                            className="text-sm leading-relaxed text-muted-foreground"
                            role="status"
                            aria-live="polite"
                          >
                            {t('resume.disconnectedDescription', { ns: 'terminal' })}
                          </p>
                        </div>
                        <button
                          type="button"
                          className="inline-flex min-h-9 items-center justify-center gap-2 bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
                          disabled={isRetrying}
                          onClick={() => void retryDisconnectedTerminal(terminal.id)}
                        >
                          <RefreshCcw
                            className={cn('h-4 w-4', isRetrying && 'animate-spin')}
                            aria-hidden="true"
                          />
                          {isRetrying
                            ? t('resume.retrying', { ns: 'terminal' })
                            : t('resume.retry', { ns: 'terminal' })}
                        </button>
                      </div>
                    </section>
                  )
                }
                // CRITICAL: Only skip rendering if terminal doesn't have a PTY
                // ID yet — this prevents spawn loops when workspace tabs aren't
                // fully synced. For agent terminals with a ptyId, ConnectedTerminal
                // MUST mount so it can attach the renderer (which sets
                // rendererAttachmentCount to 1). Instead of blocking the mount,
                // we overlay the agent loading icon on top until the renderer
                // attaches — see the overlay div after ConnectedTerminal.
                if (!terminal.ptyId) {
                  const isAgent = terminal.kind === 'agent' && !!terminal.agentId
                  return (
                    <div
                      key={tab.id}
                      className={
                        isVisible
                          ? 'w-full h-full flex flex-col items-center justify-center gap-3'
                          : 'hidden'
                      }
                    >
                      {isAgent ? (
                        <>
                          <span className="animate-pulse motion-reduce:animate-none">
                            <AgentIcon agentId={terminal.agentId!} className="h-16 w-16" />
                          </span>
                          <span className="text-sm text-muted-foreground">
                            {t('pane.starting', { name: terminal.agentName ?? terminal.name })}
                          </span>
                        </>
                      ) : (
                        <span className="text-sm text-muted-foreground">
                          {t('pane.connecting')}
                        </span>
                      )}
                    </div>
                  )
                }
                const isAgentLoading =
                  terminal.kind === 'agent' &&
                  !!terminal.agentId &&
                  agentLoadingIds.has(terminal.id)
                const connectedTerminalSpawnOptions = {
                  conversationId: terminal.conversationId,
                  projectId: terminal.projectId,
                  shell: terminal.shell,
                  cwd: terminal.cwd
                }
                return (
                  <div
                    key={tab.id}
                    className={cn(
                      isVisible ? 'w-full h-full' : INACTIVE_TAB_PANE_CLASS,
                      // A quiet inset warning keeps a finished background terminal
                      // visible without competing with pane focus or drop previews.
                      terminal.needsAttention && 'shadow-[inset_0_0_0_1px_hsl(var(--warning)/0.65)]'
                    )}
                  >
                    <Suspense fallback={<PaneSkeleton />}>
                      <ConnectedTerminal
                        terminalId={terminal.ptyId}
                        storeTerminalId={terminal.id}
                        autoSpawn={false}
                        spawnOptions={connectedTerminalSpawnOptions}
                        onBoundToStoreTerminal={(ptyId) => {
                          if (terminal.ptyId !== ptyId) {
                            setTerminalPtyId(terminal.id, ptyId)
                          }
                        }}
                        initialScrollback={terminal.pendingScrollback}
                        initialModes={terminal.pendingModes}
                        className="w-full h-full"
                        isVisible={isVisible}
                      />
                    </Suspense>
                    {/* Agent loading overlay: shown until ConnectedTerminal attaches
											the renderer (rendererAttachmentCount flips to 1). Covers the
											xterm div with a centered pulsing icon so the user sees feedback. */}
                    {isAgentLoading && (
                      <div className="absolute inset-0 z-10 flex flex-col items-center justify-center gap-3 bg-background/95">
                        <span className="animate-pulse motion-reduce:animate-none">
                          <AgentIcon
                            agentId={terminal.agentId!}
                            name={terminal.agentName}
                            className="h-16 w-16"
                          />
                        </span>
                        <span className="text-sm text-muted-foreground">
                          {t('pane.starting', { name: terminal.agentName ?? terminal.name })}
                        </span>
                      </div>
                    )}
                  </div>
                )
              })}

            {pane.tabs
              .filter((t): t is WorkspaceTab & { type: 'editor' } => t.type === 'editor')
              .map((tab) => {
                const isVisible = activeTab?.id === tab.id
                return (
                  <div
                    key={tab.id}
                    className={isVisible ? 'w-full h-full' : INACTIVE_TAB_PANE_CLASS}
                  >
                    <Suspense fallback={<PaneSkeleton />}>
                      <EditorPanel filePath={tab.filePath} isVisible={isVisible} />
                    </Suspense>
                  </div>
                )
              })}

            {pane.tabs
              .filter((t): t is WorkspaceTab & { type: 'browser' } => t.type === 'browser')
              .map((tab) => {
                const isVisible = activeTab?.id === tab.id
                return (
                  <div
                    key={tab.id}
                    className={isVisible ? 'w-full h-full' : INACTIVE_TAB_PANE_CLASS}
                  >
                    <Suspense fallback={<PaneSkeleton />}>
                      <BrowserPanel browserTabId={tab.browserTabId} isVisible={isVisible} />
                    </Suspense>
                  </div>
                )
              })}

            {pane.tabs
              .filter((t): t is WorkspaceTab & { type: 'git' } => t.type === 'git')
              .map((tab) => {
                const isVisible = activeTab?.id === tab.id
                return (
                  <div
                    key={tab.id}
                    className={isVisible ? 'w-full h-full' : INACTIVE_TAB_PANE_CLASS}
                  >
                    <Suspense fallback={<PaneSkeleton />}>
                      <GitPanel cwd={tab.cwd} isVisible={isVisible} />
                    </Suspense>
                  </div>
                )
              })}

            {pane.tabs
              .filter((t): t is WorkspaceTab & { type: 'git-history' } => t.type === 'git-history')
              .map((tab) => {
                const isVisible = activeTab?.id === tab.id
                return (
                  <div
                    key={tab.id}
                    className={isVisible ? 'w-full h-full' : INACTIVE_TAB_PANE_CLASS}
                  >
                    <Suspense fallback={<PaneSkeleton />}>
                      <GitHistoryPanel cwd={tab.cwd} isVisible={isVisible} />
                    </Suspense>
                  </div>
                )
              })}

            {pane.tabs
              .filter((t): t is WorkspaceTab & { type: 'agent-chat' } => t.type === 'agent-chat')
              .map((tab) => {
                const isVisible = activeTab?.id === tab.id
                return (
                  <div
                    key={tab.id}
                    className={isVisible ? 'w-full h-full' : INACTIVE_TAB_PANE_CLASS}
                  >
                    <Suspense fallback={<PaneSkeleton />}>
                      <ConversationAgentChatPanel
                        tab={tab}
                        paneId={pane.id}
                        isVisible={isVisible}
                      />
                    </Suspense>
                  </div>
                )
              })}

            {(() => {
              const hasRenderableContent = pane.tabs.some((tab) => {
                if (tab.type !== 'terminal') return true
                return terminalsInPane.some((terminal) => terminal.id === tab.terminalId)
              })
              if (hasRenderableContent) return null
              if (isConversationWorkspace) {
                return (
                  <div className="absolute inset-0">
                    <AgentLauncher paneId={pane.id} />
                  </div>
                )
              }
              if (!handleAddTerminalForPane) return null
              return (
                <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-background">
                  <button
                    type="button"
                    className="border border-border/60 bg-card px-3 py-1.5 text-sm text-foreground transition-colors hover:bg-secondary"
                    onClick={() => handleAddTerminalForPane()}
                  >
                    {t('emptyPane.create', { ns: 'terminal' })}
                  </button>
                </div>
              )
            })()}
          </div>
        </div>

        {isDragging && !isFullscreenPane && <DropZoneOverlay paneId={pane.id} />}
      </div>

      {/* ADR-004.5 overlay: pane-level so Ctrl+T covers tab bar + content. */}
      {agentLauncherPaneId === pane.id && pane.tabs.length > 0 ? (
        <div
          className="absolute inset-0 z-30 flex flex-col bg-background/95 backdrop-blur-sm"
          role="dialog"
          aria-modal="true"
          aria-label={t('pane.agentLauncher')}
          onKeyDown={(e) => {
            if (e.key === 'Escape') useWorkspaceStore.getState().hideAgentLauncher()
          }}
        >
          <button
            type="button"
            className="absolute right-3 top-3 z-10 flex h-8 w-8 items-center justify-center text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
            title={t('pane.closeAgentLauncherEsc')}
            aria-label={t('pane.closeAgentLauncher')}
            onClick={() => useWorkspaceStore.getState().hideAgentLauncher()}
          >
            <X className="h-4 w-4" />
          </button>
          <AgentLauncher paneId={pane.id} />
        </div>
      ) : null}
    </div>
  )
}
