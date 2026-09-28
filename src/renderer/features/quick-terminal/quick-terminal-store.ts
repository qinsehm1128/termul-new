import type { IpcResult } from '@shared/types/ipc.types'
import type {
  QuickTerminalOpened,
  QuickTerminalRecord,
  QuickTerminalTarget
} from '@shared/types/quick-terminal.types'
import { create } from 'zustand'
import { pathBasename } from '@/components/lists'
import { runtimeT } from '@/i18n/runtime'
import { logFrontendError } from '@/lib/log-api'
import { useTerminalStore } from '@/stores/terminal-store'
import { quickTerminalApi } from './quick-terminal-api'

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
  open: (id: string, cols: number, rows: number) => Promise<IpcResult<QuickTerminalOpened>>
  rename: (id: string, title: string | null) => Promise<IpcResult<QuickTerminalRecord>>
  remove: (id: string) => Promise<IpcResult<void>>
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

export const useQuickTerminalStore = create<QuickTerminalState>((set, get) => ({
  records: [],
  loaded: false,
  loadError: null,

  load: async () => {
    const result = await quickTerminalApi.list()
    logFailure('list', result)
    if (result.success) {
      set({ records: result.data ?? [], loaded: true, loadError: null })
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

  open: async (id, cols, rows) => {
    const result = await quickTerminalApi.open(id, cols, rows)
    logFailure('open', result)
    const opened = result.success ? result.data : undefined
    if (!opened) return result
    const terminals = useTerminalStore.getState()
    // A new shell replaces whatever this quick terminal showed before.
    if (opened.spawned) terminals.forgetQuickTerminal(id)
    terminals.adoptQuickTerminal({
      quickTerminalId: id,
      ptyId: opened.terminalId,
      name: quickTerminalName(opened.record),
      cwd: opened.record.cwd
    })
    set({ records: upsert(get().records, opened.record) })
    return result
  },

  rename: async (id, title) => {
    const result = await quickTerminalApi.rename(id, title)
    logFailure('rename', result)
    const record = result.success ? result.data : undefined
    if (record) set({ records: upsert(get().records, record) })
    return result
  },

  remove: async (id) => {
    const result = await quickTerminalApi.remove(id)
    logFailure('delete', result)
    if (result.success) {
      useTerminalStore.getState().forgetQuickTerminal(id)
      set({ records: get().records.filter((record) => record.id !== id) })
    }
    return result
  }
}))

/** Reset for tests. */
export function resetQuickTerminalStore(): void {
  useQuickTerminalStore.setState({ records: [], loaded: false, loadError: null })
}
