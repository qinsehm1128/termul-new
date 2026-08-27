import type { PersistedTerminalLayout } from '@shared/types/persistence.types'
import { create } from 'zustand'
import { loadPersistedTerminals } from '@/hooks/useTerminalAutoSave'
import type { Project } from '@/types/project'

export interface LastSessionProject {
  projectId: string
  name: string
  terminalCount: number
  /** Terminal names, in persisted order. Enough to recognise what was running. */
  terminalNames: string[]
}

export interface LastSessionSnapshot {
  /** Newest `updatedAt` across the projects below — when the app last wrote state. */
  capturedAt: string | null
  projects: LastSessionProject[]
}

interface LastSessionState {
  snapshot: LastSessionSnapshot | null
  dismissed: boolean
  /**
   * Read the terminal layout every project had on disk and keep it in memory.
   *
   * Must run before this session writes anything back, which is why the result
   * is captured once and never refreshed: `terminals/{projectId}` is a live
   * file that the autosave keeps current, so after the first terminal opens it
   * describes *this* session, not the one that was lost.
   */
  capture: (projects: readonly Project[]) => Promise<void>
  dismiss: () => void
  /** Test seam — resets the once-only guard. */
  reset: () => void
}

let capturePromise: Promise<void> | null = null

export const useLastSessionStore = create<LastSessionState>((set) => ({
  snapshot: null,
  dismissed: false,

  capture: (projects: readonly Project[]): Promise<void> => {
    // Concurrent callers share one read; a later caller never re-reads, because
    // by then the files may already describe the current session.
    if (capturePromise) return capturePromise

    capturePromise = (async () => {
      const entries = await Promise.all(
        projects.map(async (project) => {
          // One unreadable project must not cost the whole snapshot, and this
          // is called fire-and-forget, so it must never reject: a rejection
          // here would surface as an unhandled promise rejection rather than a
          // missing row in a notice nobody has looked at yet.
          //
          // try/catch rather than `.catch()` on the returned value — the loader
          // can throw synchronously, and then there is no promise to attach to.
          let layout: PersistedTerminalLayout | null = null
          try {
            layout = await loadPersistedTerminals(project.id)
          } catch {
            return null
          }
          if (!layout || layout.terminals.length === 0) return null
          return {
            projectId: project.id,
            name: project.name,
            terminalCount: layout.terminals.length,
            terminalNames: layout.terminals.map((terminal) => terminal.name),
            updatedAt: layout.updatedAt
          }
        })
      )

      const found = entries.filter((entry): entry is NonNullable<typeof entry> => entry !== null)
      set({
        snapshot: {
          capturedAt: found.reduce<string | null>(
            (latest, entry) =>
              latest === null || entry.updatedAt > latest ? entry.updatedAt : latest,
            null
          ),
          projects: found.map(({ updatedAt: _updatedAt, ...project }) => project)
        }
      })
    })()

    return capturePromise
  },

  dismiss: (): void => set({ dismissed: true }),

  reset: (): void => {
    capturePromise = null
    set({ snapshot: null, dismissed: false })
  }
}))

/** Total terminals across the snapshot; 0 when there is nothing worth showing. */
export function countSnapshotTerminals(snapshot: LastSessionSnapshot | null): number {
  if (!snapshot) return 0
  return snapshot.projects.reduce((total, project) => total + project.terminalCount, 0)
}
