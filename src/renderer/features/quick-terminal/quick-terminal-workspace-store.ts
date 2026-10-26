import type { QuickTerminalRecord } from '@shared/types/quick-terminal.types'
import { quickTerminalIds } from '@shared/types/quick-terminal.types'
import { create } from 'zustand'
import { logFrontendError } from '@/lib/log-api'
import { persistenceApi } from '@/lib/persistence-api'

export type QuickWorkspaceTab =
  | { id: string; type: 'terminal'; terminalId?: string; create?: boolean }
  | { id: string; type: 'editor'; filePath: string }
export interface QuickWorkspaceLayout {
  tabs: QuickWorkspaceTab[]
  activeTabId: string | null
  lastTerminalTabId: string | null
}
interface WorkspaceState {
  layouts: Record<string, QuickWorkspaceLayout>
  load: (record: QuickTerminalRecord) => Promise<void>
  update: (id: string, update: (layout: QuickWorkspaceLayout) => QuickWorkspaceLayout) => void
  reconcile: (record: QuickTerminalRecord) => void
  remove: (id: string) => Promise<void>
}
const key = (id: string): string => `quick-terminal/workspaces/${id}`
const pending = new Map<string, Promise<void>>()
const revisions = new Map<string, number>()
function warn(operation: string): void {
  void logFrontendError({
    level: 'warn',
    source: 'quick-terminal-workspace',
    message: `operation=${operation} code=PERSISTENCE_FAILED`
  })
}
function empty(): QuickWorkspaceLayout {
  return { tabs: [], activeTabId: null, lastTerminalTabId: null }
}
export function reconcileQuickWorkspace(
  layout: QuickWorkspaceLayout,
  record: QuickTerminalRecord
): QuickWorkspaceLayout {
  const ids = quickTerminalIds(record)
  const tabs = layout.tabs.filter(
    (tab) => tab.type === 'editor' || !tab.terminalId || ids.includes(tab.terminalId)
  )
  for (const terminalId of ids) {
    if (tabs.some((tab) => tab.type === 'terminal' && !tab.terminalId)) break
    if (!tabs.some((tab) => tab.type === 'terminal' && tab.terminalId === terminalId)) {
      tabs.push({ id: `terminal:${terminalId}`, type: 'terminal', terminalId })
    }
  }
  const lastTerminalTabId = tabs.some(
    (tab) => tab.id === layout.lastTerminalTabId && tab.type === 'terminal'
  )
    ? layout.lastTerminalTabId
    : (tabs.find((tab) => tab.type === 'terminal')?.id ?? null)
  const activeTabId = tabs.some((tab) => tab.id === layout.activeTabId)
    ? layout.activeTabId
    : (lastTerminalTabId ?? tabs[0]?.id ?? null)
  return { tabs, activeTabId, lastTerminalTabId }
}
function decode(value: unknown): QuickWorkspaceLayout | null {
  if (!value || typeof value !== 'object' || !('tabs' in value) || !Array.isArray(value.tabs))
    return null
  const tabs: QuickWorkspaceTab[] = []
  for (const item of value.tabs) {
    if (
      !item ||
      typeof item !== 'object' ||
      typeof item.id !== 'string' ||
      tabs.some((tab) => tab.id === item.id)
    )
      continue
    if (item.type === 'terminal' && typeof item.terminalId === 'string')
      tabs.push({ id: item.id, type: 'terminal', terminalId: item.terminalId })
    if (item.type === 'editor' && typeof item.filePath === 'string')
      tabs.push({ id: item.id, type: 'editor', filePath: item.filePath })
  }
  const data = value as Record<string, unknown>
  return {
    tabs,
    activeTabId: typeof data.activeTabId === 'string' ? data.activeTabId : null,
    lastTerminalTabId: typeof data.lastTerminalTabId === 'string' ? data.lastTerminalTabId : null
  }
}
function persist(id: string, layout: QuickWorkspaceLayout): void {
  // Never persist ephemeral spawn requests, output claims, or shell data.
  const saved = {
    ...layout,
    tabs: layout.tabs.filter((tab) => tab.type === 'editor' || !!tab.terminalId)
  }
  void persistenceApi
    .writeDebounced(key(id), saved)
    .then((result) => {
      if (!result.success) warn('write')
    })
    .catch(() => warn('write'))
}
export const useQuickWorkspaceStore = create<WorkspaceState>((set, get) => ({
  layouts: {},
  load: async (record) => {
    if (get().layouts[record.id]) {
      get().reconcile(record)
      return
    }
    const existing = pending.get(record.id)
    if (existing) return existing
    const revision = revisions.get(record.id) ?? 0
    const request = (async () => {
      let saved: QuickWorkspaceLayout | null = null
      try {
        const result = await persistenceApi.read<unknown>(key(record.id))
        if (result.success) saved = decode(result.data)
        else warn('load')
      } catch {
        warn('load')
      }
      if ((revisions.get(record.id) ?? 0) !== revision) return
      let layout = reconcileQuickWorkspace(saved ?? empty(), record)
      if (!saved && layout.tabs.length === 0) {
        layout = {
          tabs: [{ id: 'initial', type: 'terminal' }],
          activeTabId: 'initial',
          lastTerminalTabId: 'initial'
        }
      }
      set((state) => ({ layouts: { ...state.layouts, [record.id]: layout } }))
    })()
    pending.set(record.id, request)
    try {
      await request
    } finally {
      pending.delete(record.id)
    }
  },
  update: (id, update) => {
    const previous = get().layouts[id]
    if (!previous) return
    const layout = update(previous)
    set((state) => ({ layouts: { ...state.layouts, [id]: layout } }))
    persist(id, layout)
  },
  reconcile: (record) => {
    const previous = get().layouts[record.id]
    if (!previous) return
    const layout = reconcileQuickWorkspace(previous, record)
    if (JSON.stringify(layout) === JSON.stringify(previous)) return
    set((state) => ({ layouts: { ...state.layouts, [record.id]: layout } }))
    persist(record.id, layout)
  },
  remove: async (id) => {
    revisions.set(id, (revisions.get(id) ?? 0) + 1)
    set((state) => {
      const layouts = { ...state.layouts }
      delete layouts[id]
      return { layouts }
    })
    try {
      // Flush queued writes first, otherwise a late debounced write resurrects a deleted layout.
      const flushed = await persistenceApi.flushPendingWrites()
      if (!flushed.success) warn('flush')
      const result = await persistenceApi.delete(key(id))
      if (!result.success) warn('delete')
    } catch {
      warn('delete')
    }
  }
}))
export function resetQuickWorkspaceStore(): void {
  pending.clear()
  revisions.clear()
  useQuickWorkspaceStore.setState({ layouts: {} })
}
