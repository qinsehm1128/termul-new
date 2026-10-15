import { create } from 'zustand'

export interface CommandHistoryEntry {
  id: string
  command: string
  terminalName: string
  terminalId: string
  projectId: string
  timestamp: number
}

interface CommandHistoryState {
  entries: CommandHistoryEntry[]
  addCommand: (entry: Omit<CommandHistoryEntry, 'id'>) => void
  clearHistory: (projectId: string) => void
  setHistory: (entries: CommandHistoryEntry[]) => void
}

const MAX_HISTORY_ENTRIES = 500

/**
 * Most command text kept per project. Prompts pasted into agents run to tens
 * of KB each, so the entry cap alone let one project's history reach ~800 KB,
 * all of it rewritten on every new entry.
 */
export const MAX_HISTORY_CHARS = 256 * 1024

/** Newest-first entries, cut where the budget runs out. The newest always stays. */
function withinBudget(entries: CommandHistoryEntry[]): CommandHistoryEntry[] {
  let used = 0
  for (let index = 0; index < entries.length; index++) {
    used += entries[index].command.length
    if (used > MAX_HISTORY_CHARS && index > 0) return entries.slice(0, index)
  }
  return entries
}

export const useCommandHistoryStore = create<CommandHistoryState>((set) => ({
  entries: [],

  addCommand: (entry) =>
    set((state) => {
      const newEntry: CommandHistoryEntry = {
        ...entry,
        id: `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`
      }
      // Add to front, limit total entries
      const filtered = state.entries.filter((e) => e.projectId === entry.projectId)
      const otherProjects = state.entries.filter((e) => e.projectId !== entry.projectId)
      const projectEntries = withinBudget([newEntry, ...filtered].slice(0, MAX_HISTORY_ENTRIES))
      return {
        entries: [...projectEntries, ...otherProjects]
      }
    }),

  clearHistory: (projectId) =>
    set((state) => ({
      entries: state.entries.filter((e) => e.projectId !== projectId)
    })),

  setHistory: (entries) => set({ entries })
}))
