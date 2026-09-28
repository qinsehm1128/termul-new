import {
  parseQuickTerminalOpened,
  parseQuickTerminalRecord,
  parseQuickTerminalRecords,
  type QuickTerminalApi
} from '@shared/types/quick-terminal.types'
import { onHostEvent } from '@/lib/host-events'
import { invokeDecodedIpcResult } from '@/lib/invoke-decoded-ipc-result'

const decodeVoid = (): void => undefined

export function createTauriQuickTerminalApi(): QuickTerminalApi {
  return {
    list: () => invokeDecodedIpcResult('quick_terminal_list', parseQuickTerminalRecords),
    create: (input) =>
      invokeDecodedIpcResult('quick_terminal_create', parseQuickTerminalRecord, {
        payload: { target: input.target, title: input.title ?? null }
      }),
    open: (id, cols, rows) =>
      invokeDecodedIpcResult('quick_terminal_open', parseQuickTerminalOpened, {
        payload: { id, cols, rows }
      }),
    rename: (id, title) =>
      invokeDecodedIpcResult('quick_terminal_rename', parseQuickTerminalRecord, {
        payload: { id, title }
      }),
    remove: (id) =>
      invokeDecodedIpcResult('quick_terminal_delete', decodeVoid, { payload: { id } }),
    onChanged: (handler) => onHostEvent('quick-terminals-changed', () => handler())
  }
}

export const tauriQuickTerminalApi = createTauriQuickTerminalApi()
