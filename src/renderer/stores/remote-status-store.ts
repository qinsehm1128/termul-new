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

/**
 * Whether two status payloads carry the same values.
 *
 * The desktop polls `remote_server_status` every 3 s, and the backend builds a
 * fresh object each time. `set({ status })` therefore notified every subscriber
 * 1200 times an hour while nothing had changed. Every field is a primitive, so
 * a shallow compare over the union of keys is exact.
 */
function sameRemoteStatus(a: RemoteStatus | null, b: RemoteStatus | null): boolean {
  if (a === b) return true
  if (!a || !b) return false
  const keys = Object.keys(a) as (keyof RemoteStatus)[]
  if (keys.length !== Object.keys(b).length) return false
  for (const key of keys) {
    if (a[key] !== b[key]) return false
  }
  return true
}

export const useRemoteStatusStore = create<RemoteStatusStore>((set) => ({
  status: null,
  restoreError: null,
  // Returning the same state object makes zustand skip the notification, so a
  // poll that learned nothing new no longer re-renders the whole shell.
  setStatus: (status) =>
    set((state) => (sameRemoteStatus(state.status, status) ? state : { status })),
  setRestoreError: (restoreError) =>
    set((state) => (state.restoreError === restoreError ? state : { restoreError }))
}))

/** Selector: current remote status (or null). */
export const useRemoteStatus = (): RemoteStatus | null => useRemoteStatusStore((s) => s.status)

export const useRemoteRestoreError = (): string | null =>
  useRemoteStatusStore((s) => s.restoreError)
