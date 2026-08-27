import { useEffect } from 'react'
import { toProjectGroupSummaries, toProjectSummaries } from '@/hooks/use-projects-persistence'
import { remoteServerApi, syncProjects } from '@/lib/api'
import { logFrontendError } from '@/lib/log-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { useProjectStore } from '@/stores/project-store'
import { useRemoteStatusStore } from '@/stores/remote-status-store'

/**
 * Polls the desktop-hosted web server status and restores a wanted session
 * once after launch. Mounted once near the app root. No-op outside Tauri.
 */
export function useRemoteProjects(): void {
  useEffect(() => {
    if (!isTauriContext()) return

    let disposed = false
    let restoreAttempted = false

    const pollStatus = async (): Promise<void> => {
      if (disposed) return
      const result = await remoteServerApi.status()
      if (!disposed && result.success) {
        useRemoteStatusStore.getState().setStatus(result.data)
      }
    }

    const restoreWanted = async (): Promise<void> => {
      if (disposed || restoreAttempted) return
      restoreAttempted = true
      const intent = await remoteServerApi.intent()
      if (!intent.success || !intent.data.wanted) return
      const current = await remoteServerApi.status()
      if (current.success && current.data.running) {
        useRemoteStatusStore.getState().setStatus(current.data)
        return
      }
      const bindMode = intent.data.publishMode === 'lan' ? 'all' : 'localhost'
      const started = await remoteServerApi.start({ bindMode })
      if (disposed) return
      if (started.success) {
        useRemoteStatusStore.getState().setStatus(started.data)
        useRemoteStatusStore.getState().setRestoreError(null)
        const { projects, groups, activeProjectId } = useProjectStore.getState()
        const syncResult = await syncProjects(
          toProjectSummaries(projects, activeProjectId),
          activeProjectId || null,
          toProjectGroupSummaries(groups)
        )
        if (!syncResult.success) {
          void logFrontendError({
            level: 'warn',
            source: 'useRemoteProjects.restore',
            message: `Failed to seed remote project list: ${syncResult.error}`
          })
        }
        return
      }
      useRemoteStatusStore.getState().setRestoreError(started.error)
      void logFrontendError({
        level: 'error',
        source: 'useRemoteProjects.restore',
        message: `Failed to restore remote access: ${started.error}`
      })
    }

    void restoreWanted()
    void pollStatus()
    const statusTimer = setInterval(() => void pollStatus(), 3000)

    return () => {
      disposed = true
      clearInterval(statusTimer)
    }
  }, [])
}
