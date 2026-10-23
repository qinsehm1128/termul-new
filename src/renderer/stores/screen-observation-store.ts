import type {
  ScreenObservationStatus,
  TerminalScreenEngine
} from '@shared/types/terminal-screen.types'
import { normalizeScreenObservation } from '@shared/types/terminal-screen.types'
import { create } from 'zustand'

export interface HostScreenObservation {
  /** Actual immutable spawn/adopt mode from the host, never a project setting. */
  terminalScreenEngine?: TerminalScreenEngine
  screenObservation?: ScreenObservationStatus
  status?: ScreenObservationStatus
  claim?: string
}

interface ScreenObservationStore {
  byPtyId: Record<string, HostScreenObservation>
  recordHost: (ptyId: string, host: HostScreenObservation) => void
  clear: (ptyId: string) => void
}

/** Transient host evidence only; no local persistence and no authorization decisions. */
export const useScreenObservationStore = create<ScreenObservationStore>((set) => ({
  byPtyId: {},
  recordHost: (ptyId, host) =>
    set((state) => {
      const previous = state.byPtyId[ptyId]
      const rotated = host.claim !== undefined && host.claim !== previous?.claim
      const terminalScreenEngine = host.terminalScreenEngine ?? previous?.terminalScreenEngine
      const incoming = host.status ?? host.screenObservation
      const hostStatus = incoming === undefined ? undefined : normalizeScreenObservation(incoming)
      const status =
        !rotated && previous?.status?.state === 'degraded'
          ? previous.status
          : (hostStatus ?? (rotated ? undefined : previous?.status))
      if (
        previous &&
        previous.terminalScreenEngine === terminalScreenEngine &&
        previous.status === status &&
        (host.claim === undefined || host.claim === previous.claim)
      )
        return state
      return {
        byPtyId: {
          ...state.byPtyId,
          [ptyId]: {
            terminalScreenEngine,
            status,
            claim: host.claim ?? previous?.claim
          }
        }
      }
    }),
  clear: (ptyId) =>
    set((state) => {
      if (!state.byPtyId[ptyId]) return state
      const byPtyId = { ...state.byPtyId }
      delete byPtyId[ptyId]
      return { byPtyId }
    })
}))
