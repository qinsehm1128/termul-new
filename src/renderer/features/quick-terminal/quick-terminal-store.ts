import type { IpcResult } from '@shared/types/ipc.types'
import type {
  QuickTerminalOpened,
  QuickTerminalRecord,
  QuickTerminalTarget
} from '@shared/types/quick-terminal.types'
import { quickTerminalIds } from '@shared/types/quick-terminal.types'
import { create } from 'zustand'
import { pathBasename } from '@/components/lists'
import { runtimeT } from '@/i18n/runtime'
import { logFrontendError } from '@/lib/log-api'
import { useTerminalStore } from '@/stores/terminal-store'
import { quickTerminalApi } from './quick-terminal-api'
import { useQuickWorkspaceStore } from './quick-terminal-workspace-store'

interface QuickTerminalState {
  records: QuickTerminalRecord[]
  loaded: boolean
  loadError: string | null
  load: () => Promise<void>
  create: (target: QuickTerminalTarget) => Promise<IpcResult<QuickTerminalRecord>>
  /**
   * Show the quick terminal's shell: reuse the live PTY or let the host start
   * a new one, and register it so `ConnectedTerminal` can attach.
   */
  open: (
    id: string,
    cols: number,
    rows: number,
    terminalId?: string
  ) => Promise<IpcResult<QuickTerminalOpened>>
  createTerminal: (
    id: string,
    cols: number,
    rows: number
  ) => Promise<IpcResult<QuickTerminalOpened>>
  closeTerminal: (id: string, terminalId: string) => Promise<IpcResult<QuickTerminalRecord>>
  rename: (id: string, title: string | null) => Promise<IpcResult<QuickTerminalRecord>>
  remove: (id: string) => Promise<IpcResult<void>>
  /** End the shell, keep the quick terminal. */
  close: (id: string) => Promise<IpcResult<QuickTerminalRecord>>
}

function logFailure(operation: string, result: IpcResult<unknown>): void {
  if (result.success) return
  void logFrontendError({
    level: 'warn',
    source: 'quick-terminal-store',
    message: `operation=${operation} code=${result.code || 'UNKNOWN'}`
  })
}

function upsert(
  records: QuickTerminalRecord[],
  record: QuickTerminalRecord
): QuickTerminalRecord[] {
  const rest = records.filter((candidate) => candidate.id !== record.id)
  return [record, ...rest].sort((left, right) =>
    right.updatedAtUtc.localeCompare(left.updatedAtUtc)
  )
}

/** The host's message for a failed result, else `fallback`. */
export function failureMessage(result: IpcResult<unknown>, fallback: string): string {
  return result.success ? fallback : result.error || fallback
}

/**
 * Display name: the user's title; else, for a project shell, the folder it
 * runs in. A private folder is named by its id, so it reads as untitled.
 */
export function quickTerminalName(record: QuickTerminalRecord): string {
  const title = record.title?.trim()
  if (title) return title
  if (record.target.kind === 'workspace') {
    return runtimeT('quickTerminal', 'untitled', 'Untitled terminal')
  }
  return pathBasename(record.cwd) || record.cwd
}

/** Drop only this entry's PTYs that the host no longer owns, retaining siblings. */
function pruneUnrecordedTerminals(id: string, record: QuickTerminalRecord): void {
  const alive = quickTerminalIds(record)
  useTerminalStore.setState((state) => {
    const removed = state.terminals.filter(
      (item) => item.quickTerminalId === id && !alive.includes(item.ptyId ?? item.id)
    )
    const ptyIdIndex = new Map(state.ptyIdIndex)
    for (const item of removed) if (item.ptyId) ptyIdIndex.delete(item.ptyId)
    return { terminals: state.terminals.filter((item) => !removed.includes(item)), ptyIdIndex }
  })
}

// Host operations for one entry are ordered independently of page mounts.
// Otherwise two concurrent opens can publish older record snapshots over a
// newer create/close response when the user changes tabs or entries quickly.
const entryOperations = new Map<string, Promise<unknown>>()
function inEntryOrder<T>(id: string, action: () => Promise<T>): Promise<T> {
  const previous = entryOperations.get(id) ?? Promise.resolve()
  const request = previous.catch(() => undefined).then(action)
  entryOperations.set(id, request)
  void request
    .finally(() => {
      if (entryOperations.get(id) === request) entryOperations.delete(id)
    })
    .catch(() => undefined)
  return request
}
let recordsRevision = 0
let listRequest = 0

export const useQuickTerminalStore = create<QuickTerminalState>((set, get) => ({
  records: [],
  loaded: false,
  loadError: null,

  load: async () => {
    const request = ++listRequest
    const revision = recordsRevision
    const result = await quickTerminalApi.list()
    if (request !== listRequest || revision !== recordsRevision) return
    logFailure('list', result)
    if (result.success) {
      set({ records: result.data ?? [], loaded: true, loadError: null })
      for (const record of result.data ?? []) useQuickWorkspaceStore.getState().reconcile(record)
    } else {
      set({ loaded: true, loadError: failureMessage(result, result.code ?? 'error') })
    }
  },

  create: async (target) => {
    const result = await quickTerminalApi.create({ target })
    logFailure('create', result)
    const record = result.success ? result.data : undefined
    if (record) set({ records: upsert(get().records, record) })
    return result
  },

  open: (id, cols, rows, terminalId) =>
    inEntryOrder(id, async () => {
      recordsRevision += 1
      const result = terminalId
        ? await quickTerminalApi.open(id, cols, rows, terminalId)
        : await quickTerminalApi.open(id, cols, rows)
      recordsRevision += 1
      logFailure('open', result)
      const opened = result.success ? result.data : undefined
      if (!opened) return result
      pruneUnrecordedTerminals(id, opened.record)
      useTerminalStore.getState().adoptQuickTerminal({
        quickTerminalId: id,
        ptyId: opened.terminalId,
        name: quickTerminalName(opened.record),
        cwd: opened.record.cwd
      })
      set({ records: upsert(get().records, opened.record) })
      return result
    }),

  createTerminal: (id, cols, rows) =>
    inEntryOrder(id, async () => {
      recordsRevision += 1
      const result = await quickTerminalApi.createTerminal(id, cols, rows)
      recordsRevision += 1
      logFailure('create-terminal', result)
      if (result.success) {
        pruneUnrecordedTerminals(id, result.data.record)
        useTerminalStore.getState().adoptQuickTerminal({
          quickTerminalId: id,
          ptyId: result.data.terminalId,
          name: quickTerminalName(result.data.record),
          cwd: result.data.record.cwd
        })
        set({ records: upsert(get().records, result.data.record) })
      }
      return result
    }),

  closeTerminal: (id, terminalId) =>
    inEntryOrder(id, async () => {
      recordsRevision += 1
      const result = await quickTerminalApi.closeTerminal(id, terminalId)
      recordsRevision += 1
      logFailure('close-terminal', result)
      if (result.success) {
        useTerminalStore.setState((state) => {
          const ptyIdIndex = new Map(state.ptyIdIndex)
          ptyIdIndex.delete(terminalId)
          return { terminals: state.terminals.filter((item) => item.id !== terminalId), ptyIdIndex }
        })
        set({ records: upsert(get().records, result.data) })
        useQuickWorkspaceStore.getState().reconcile(result.data)
      }
      return result
    }),

  rename: async (id, title) => {
    const result = await quickTerminalApi.rename(id, title)
    logFailure('rename', result)
    const record = result.success ? result.data : undefined
    if (record) set({ records: upsert(get().records, record) })
    return result
  },

  close: (id) =>
    inEntryOrder(id, async () => {
      recordsRevision += 1
      const result = await quickTerminalApi.close(id)
      recordsRevision += 1
      logFailure('close', result)
      const record = result.success ? result.data : undefined
      if (record) {
        useTerminalStore.getState().forgetQuickTerminal(id)
        set({ records: upsert(get().records, record) })
        useQuickWorkspaceStore.getState().reconcile(record)
      }
      return result
    }),

  remove: (id) =>
    inEntryOrder(id, async () => {
      recordsRevision += 1
      const result = await quickTerminalApi.remove(id)
      recordsRevision += 1
      logFailure('delete', result)
      if (result.success) {
        useTerminalStore.getState().forgetQuickTerminal(id)
        set({ records: get().records.filter((record) => record.id !== id) })
        await useQuickWorkspaceStore.getState().remove(id)
      }
      return result
    })
}))

/** Reset for tests. */
export function resetQuickTerminalStore(): void {
  entryOperations.clear()
  recordsRevision = 0
  listRequest = 0
  useQuickTerminalStore.setState({ records: [], loaded: false, loadError: null })
}
