import { useEffect } from 'react'
import { performSessionWorkspaceWrite } from '@/hooks/use-session-workspace-sync'
import { terminalApi } from '@/lib/terminal-api'
import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'

/**
 * Reconciles renderer view state with Conversation-scoped terminal resources.
 *
 * Cleanup only removes event subscriptions. Root unmount, navigation, project
 * switching, and transport disconnect never call terminate/kill or remove a
 * passive SessionWorkspace reference.
 */
export function useTerminalResourceLifecycle(): void {
  const conversationId = useSessionWorkspaceSyncStore((state) => state.activeConversationId)

  useEffect(() => {
    if (!conversationId) return

    const terminals = useTerminalStore
      .getState()
      .terminals.filter((terminal) => terminal.conversationId === conversationId)
    for (const terminal of terminals) {
      if (terminal.viewState === 'visible') {
        useWorkspaceStore.getState().ensureTerminalTab(terminal.id, undefined, false)
      } else {
        useWorkspaceStore.getState().closeTerminalView(terminal.id)
      }
    }

    const offExit = terminalApi.onExit((ptyId, exitCode) => {
      const terminal = useTerminalStore.getState().findTerminalByPtyId(ptyId)
      if (!terminal || terminal.conversationId !== conversationId) return
      const store = useTerminalStore.getState()
      store.updateTerminalExitCode(terminal.id, exitCode)
      store.setTerminalClaim(ptyId, undefined)
      store.setTerminalHealthStatus(
        terminal.id,
        terminal.healthStatus === 'disconnected' ? 'disconnected' : 'crashed'
      )
      void performSessionWorkspaceWrite(conversationId)
    })

    return () => {
      offExit()
    }
  }, [conversationId])
}
