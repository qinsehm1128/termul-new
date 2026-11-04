import { quickTerminalIds } from '@shared/types/quick-terminal.types'
import { useEffect } from 'react'
import { useAcpStore } from '@/features/agent-session/stores/acp-store'
import { useQuickTerminalStore } from '@/features/quick-terminal/quick-terminal-store'
import { useQuickWorkspaceStore } from '@/features/quick-terminal/quick-terminal-workspace-store'
import { isTerminalRestoreInProgress } from '@/hooks/useTerminalAutoSave'
import { isTauriContext } from '@/lib/tauri-runtime'
import { randomUUID } from '@/lib/uuid'
import { usePluginsStore } from '@/stores/plugins-store'
import { useProjectStore } from '@/stores/project-store'
import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceManifestSyncStore } from '@/stores/workspace-manifest-sync-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import { createHostContextPublisher, projectHostContext } from './host-context'
import { usePluginHostUiStore } from './host-ui-store'
import { pluginApi } from './plugin-api'

function focusContext() {
  const element = document.activeElement
  const terminal = !!element?.closest('.xterm, [data-terminal-id]')
  return {
    terminal,
    textInput: !terminal && !!element?.closest('input, textarea, select, [contenteditable="true"]')
  }
}

function displayedPath(): string {
  return new URL(window.location.hash.slice(1) || '/', window.location.origin).pathname
}

function displayedTab() {
  const path = displayedPath()
  // These are the routes that render PaneRenderer in WorkspaceLayout. `/workspace`
  // is not a route; `/c/:conversationId` also renders the primary pane tree.
  if (path === '/' || path === '/terminal' || /^\/c\/[^/]+\/?$/.test(path))
    return useWorkspaceStore.getState().getActiveTab()
  const match = /^\/quick-terminals\/([^/]+)\/?$/.exec(path)
  if (!match) return undefined
  let id: string
  try {
    id = decodeURIComponent(match[1])
  } catch {
    return undefined
  }
  const record = useQuickTerminalStore.getState().records.find((item) => item.id === id)
  const layout = useQuickWorkspaceStore.getState().layouts[id]
  const tab = layout?.tabs.find((item) => item.id === layout.activeTabId)
  if (
    !record ||
    tab?.type !== 'terminal' ||
    !tab.terminalId ||
    !quickTerminalIds(record).includes(tab.terminalId)
  )
    return undefined
  return tab
}

export function readPluginHostContext() {
  const project = useProjectStore.getState()
  const terminals = useTerminalStore.getState()
  const acp = useAcpStore.getState()
  const activeTab = displayedTab()
  const activeSessionId =
    activeTab?.type === 'agent-chat'
      ? (activeTab.sessionId ??
        Object.values(acp.sessions).find(
          (session) => session.conversationId === activeTab.conversationId
        )?.id ??
        null)
      : null
  return projectHostContext({
    projects: project.projects,
    activeProjectId: project.activeProjectId,
    terminals: terminals.terminals,
    sessions: Object.values(acp.sessions),
    activeTerminalId: activeTab?.type === 'terminal' ? (activeTab.terminalId ?? null) : null,
    activeSessionId,
    focus: focusContext()
  })
}

/** Mounted once at the portable root; browser mounts register no native side effects. */
export function usePluginHostContext(): void {
  useEffect(() => {
    if (!isTauriContext()) return
    const publisher = createHostContextPublisher({
      api: pluginApi,
      sourceId: randomUUID(),
      initial: readPluginHostContext(),
      onUnavailable: () => usePluginHostUiStore.getState().setContextPending(true),
      onAck: (ack, snapshot) => {
        const store = usePluginHostUiStore.getState()
        store.acknowledgeContext(ack.contextRevision, snapshot)
        if (snapshot)
          store.setContextPending(
            JSON.stringify(snapshot) !== JSON.stringify(readPluginHostContext())
          )
      }
    })
    const restoring = () =>
      !useProjectStore.getState().isLoaded ||
      isTerminalRestoreInProgress() ||
      Object.values(useSessionWorkspaceSyncStore.getState().restoreInProgressByConversation).some(
        Boolean
      ) ||
      Object.values(
        useWorkspaceManifestSyncStore.getState().manifestRestoreInProgressByProject
      ).some(Boolean)
    let wasRestoring = restoring()
    let lastContext = JSON.stringify(readPluginHostContext())
    const capture = (reset = false) => {
      const nextRestoring = restoring()
      const snapshot = readPluginHostContext()
      const encoded = JSON.stringify(snapshot)
      if (encoded !== lastContext || reset || nextRestoring || wasRestoring)
        usePluginHostUiStore.getState().setContextPending(true)
      lastContext = encoded
      publisher.capture(snapshot, reset || nextRestoring || wasRestoring)
      wasRestoring = nextRestoring
    }
    let lastPath = displayedPath()
    const onRoute = () => {
      const path = displayedPath()
      if (path === lastPath) return
      lastPath = path
      capture(true)
    }
    // PortableAppEffects sits outside RouterProvider. Hash-router push/replace
    // calls do not emit hashchange, so capture them before returning to callers.
    const originalPush = window.history.pushState
    const originalReplace = window.history.replaceState
    const push: History['pushState'] = function (this: History, ...args) {
      originalPush.apply(this, args)
      onRoute()
    }
    const replace: History['replaceState'] = function (this: History, ...args) {
      originalReplace.apply(this, args)
      onRoute()
    }
    window.history.pushState = push
    window.history.replaceState = replace
    window.addEventListener('popstate', onRoute)
    window.addEventListener('hashchange', onRoute)
    const disposers = [
      useProjectStore.subscribe((next, previous) => capture(next.isLoaded !== previous.isLoaded)),
      useTerminalStore.subscribe(() => capture()),
      useAcpStore.subscribe(() => capture()),
      useWorkspaceStore.subscribe(() => capture()),
      useQuickTerminalStore.subscribe(() => capture()),
      useQuickWorkspaceStore.subscribe(() => capture()),
      useSessionWorkspaceSyncStore.subscribe((next, previous) => {
        capture(
          next.restoreInProgressByConversation !== previous.restoreInProgressByConversation ||
            next.loadOutcomeByConversation !== previous.loadOutcomeByConversation
        )
      }),
      useWorkspaceManifestSyncStore.subscribe((next, previous) => {
        capture(
          next.manifestRestoreInProgressByProject !== previous.manifestRestoreInProgressByProject
        )
      }),
      usePluginsStore.subscribe((next, previous) => {
        if (next.connected && (!previous.connected || next.bootId !== previous.bootId))
          publisher.reconnect()
        else if (!next.connected && previous.connected) publisher.disconnect()
      })
    ]
    if (usePluginsStore.getState().connected) publisher.reconnect()
    const onFocus = () => capture()
    document.addEventListener('focusin', onFocus)
    document.addEventListener('focusout', onFocus)
    // Legacy terminal restore uses a module-local registry, not a subscribable store.
    // Observe its boundary even if no other store changes at completion.
    const restorePoll = setInterval(() => {
      if (restoring() !== wasRestoring) capture(true)
    }, 50)
    return () => {
      publisher.dispose()
      clearInterval(restorePoll)
      disposers.forEach((dispose) => {
        dispose()
      })
      document.removeEventListener('focusin', onFocus)
      document.removeEventListener('focusout', onFocus)
      window.removeEventListener('popstate', onRoute)
      window.removeEventListener('hashchange', onRoute)
      if (window.history.pushState === push) window.history.pushState = originalPush
      if (window.history.replaceState === replace) window.history.replaceState = originalReplace
    }
  }, [])
}
