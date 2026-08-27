import type { RemoteStatus } from '@shared/types/ipc.types'
import { create } from 'zustand'

/**
 * Global store for the embedded remote-terminal server status.
 *
 * Populated by `use-remote-projects` (which polls the backend) and updated
 * immediately when the user toggles remote access from the StatusBar popover.
 */
interface RemoteStatusStore {
  status: RemoteStatus | null
  restoreError: string | null
  setStatus: (status: RemoteStatus | null) => void
  setRestoreError: (error: string | null) => void
}

export const useRemoteStatusStore = create<RemoteStatusStore>((set) => ({
  status: null,
  restoreError: null,
  setStatus: (status) => set({ status }),
  setRestoreError: (restoreError) => set({ restoreError })
}))

/** Selector: current remote status (or null). */
export const useRemoteStatus = (): RemoteStatus | null => useRemoteStatusStore((s) => s.status)

export const useRemoteRestoreError = (): string | null =>
  useRemoteStatusStore((s) => s.restoreError)
