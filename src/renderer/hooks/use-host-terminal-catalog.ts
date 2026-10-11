import { useEffect } from 'react'
import { isLocalTerminalSpawnEvent } from '@/lib/local-terminal-spawns'
import { logFrontendError } from '@/lib/log-api'
import { terminalApi } from '@/lib/terminal-api'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'

/**
 * Keep the desktop/web terminal list live with PTYs created on the phone.
 * Does not steal the active tab.
 */
export function useHostTerminalCatalog(): void {
  useEffect(() => {
    if (!terminalApi.onSpawned) return undefined
    return terminalApi.onSpawned((event) => {
      if (isLocalTerminalSpawnEvent(event.terminalId)) return
      const adoptedId = useTerminalStore.getState().adoptRemoteProjectTerminal(event)
      if (!adoptedId) return
      if (useProjectStore.getState().activeProjectId === event.projectId) {
        useWorkspaceStore.getState().ensureTerminalTab(adoptedId, undefined, false)
      }
      void logFrontendError({
        level: 'warn',
        source: 'host-terminal-catalog',
        message: 'Adopted a remote project terminal into the workspace'
      })
    })
  }, [])
}
