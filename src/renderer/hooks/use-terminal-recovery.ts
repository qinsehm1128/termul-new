import { useCallback, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { useQuickTerminalStore } from '@/features/quick-terminal/quick-terminal-store'
import { runtimeT } from '@/i18n/runtime'
import { logFrontendError } from '@/lib/log-api'
import { adoptLiveTerminalsOnBoot, recoverTerminalWorkspace } from '@/lib/terminal-recovery'
import { useProjectStore } from '@/stores/project-store'

const recoveryT = (key: string, fallback: string, values?: Record<string, number>) =>
  runtimeT('shell', `recovery.${key}`, fallback, values)

// One recovery at a time across every surface that offers it.
let inFlight: Promise<void> | null = null
const listeners = new Set<(running: boolean) => void>()

function setRunning(running: boolean): void {
  for (const listener of listeners) listener(running)
}

/** Reload the page; the restore that runs on boot takes the terminals back. */
export function reloadAndRecover(): void {
  window.location.reload()
}

async function runRecovery(): Promise<void> {
  try {
    const [result] = await Promise.all([
      recoverTerminalWorkspace(),
      useQuickTerminalStore.getState().load()
    ])
    const changed = result.adopted + result.tabsRestored + result.disconnected
    if (changed === 0) {
      toast.success(recoveryT('nothing', 'Everything is already connected.'))
      return
    }
    toast.success(
      recoveryT(
        'done',
        'Restored: {{tabs}} tabs back, {{adopted}} terminals reconnected, {{disconnected}} marked disconnected.',
        {
          tabs: result.tabsRestored,
          adopted: result.adopted,
          disconnected: result.disconnected
        }
      )
    )
  } catch (error) {
    void logFrontendError({
      level: 'warn',
      source: 'terminal-recovery',
      message: error instanceof Error ? error.message : String(error)
    })
    toast.error(recoveryT('failed', 'Could not restore in place.'), {
      action: { label: recoveryT('reload', 'Reload UI and restore'), onClick: reloadAndRecover }
    })
  }
}

/** Restore the UI's terminals from Terminal Core, in place. */
export function recoverTerminals(): Promise<void> {
  if (inFlight) return inFlight
  setRunning(true)
  inFlight = runRecovery().finally(() => {
    inFlight = null
    setRunning(false)
  })
  return inFlight
}

export function useTerminalRecovery(): { recover: () => void; recovering: boolean } {
  const [recovering, setRecovering] = useState(inFlight !== null)
  useEffect(() => {
    listeners.add(setRecovering)
    return () => {
      listeners.delete(setRecovering)
    }
  }, [])
  const recover = useCallback(() => {
    void recoverTerminals()
  }, [])
  return { recover, recovering }
}

let bootAdoptionStarted = false

/** Take back every project's running terminals once, when the renderer starts. */
export function useAdoptLiveTerminalsOnBoot(): void {
  const projectsLoaded = useProjectStore((state) => state.isLoaded)
  useEffect(() => {
    if (!projectsLoaded || bootAdoptionStarted) return
    bootAdoptionStarted = true
    adoptLiveTerminalsOnBoot().catch((error: unknown) => {
      void logFrontendError({
        level: 'warn',
        source: 'terminal-recovery.boot',
        message: error instanceof Error ? error.message : String(error)
      })
    })
  }, [projectsLoaded])
}
