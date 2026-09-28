import type { IpcDataDecoder, IpcResult } from '@shared/types/ipc.types'
import {
  parseQuickTerminalOpened,
  parseQuickTerminalRecord,
  parseQuickTerminalRecords,
  type QuickTerminalApi
} from '@shared/types/quick-terminal.types'
import { requestHttpIpcResult } from '@/lib/http-ipc-result'
import { remoteAccessHeaders } from '@/lib/remote-access-credential'

function request<T>(
  path: string,
  decoder: IpcDataDecoder<T>,
  init: RequestInit = { method: 'GET' }
): Promise<IpcResult<T>> {
  return requestHttpIpcResult(
    `${typeof window === 'undefined' ? '' : window.location.origin}${path}`,
    { ...init, headers: remoteAccessHeaders(init.headers) },
    decoder
  )
}

function post<T>(path: string, body: unknown, decoder: IpcDataDecoder<T>): Promise<IpcResult<T>> {
  return request(path, decoder, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  })
}

const decodeVoid = (): void => undefined

export function createWebQuickTerminalApi(): QuickTerminalApi {
  return {
    list: () => request('/quick-terminals', parseQuickTerminalRecords),
    create: (input) =>
      post(
        '/quick-terminals',
        { target: input.target, title: input.title ?? null },
        parseQuickTerminalRecord
      ),
    open: (id, cols, rows) =>
      post('/quick-terminals/open', { id, cols, rows }, parseQuickTerminalOpened),
    rename: (id, title) => post('/quick-terminals/rename', { id, title }, parseQuickTerminalRecord),
    remove: (id) => post('/quick-terminals/delete', { id }, decodeVoid),
    // No push channel on the web; the page lists again whenever it mounts.
    onChanged: () => () => undefined
  }
}

export const webQuickTerminalApi = createWebQuickTerminalApi()
