import { useEffect } from 'react'
import { useEditorStore } from '@/stores/editor-store'
import { useTerminalStore } from '@/stores/terminal-store'
import {
  getActiveFilePathFromTree,
  getActiveTerminalIdFromTree,
  useWorkspaceStore
} from '@/stores/workspace-store'

/** Hidden project panes must not override a quick workspace's active tab. */
export function useWorkspaceActiveTabSync(enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return
    const synchronize = (): void => {
      const state = useWorkspaceStore.getState()
      const terminalId = getActiveTerminalIdFromTree(state)
      if (terminalId !== null) {
        const terminals = useTerminalStore.getState()
        if (terminals.activeTerminalId !== terminalId) terminals.selectTerminal(terminalId)
      }
      const filePath = getActiveFilePathFromTree(state)
      const editor = useEditorStore.getState()
      if (editor.activeFilePath !== filePath) editor.setActiveFilePath(filePath)
    }
    // Restore project ownership immediately when returning from a quick workspace,
    // even if its pane tree did not change while it was hidden.
    synchronize()
    return useWorkspaceStore.subscribe((state, previous) => {
      if (state.root !== previous.root || state.activePaneId !== previous.activePaneId)
        synchronize()
    })
  }, [enabled])
}
