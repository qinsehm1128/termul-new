import { decodeIpcResult, type IpcDataDecoder, type IpcResult } from '@shared/types/ipc.types'

export const HTTP_IPC_NETWORK_ERROR_MESSAGE = 'Invalid response from host'

function networkError(): IpcResult<never> {
  return {
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  }
}

/** Parse one HTTP response body exactly once and validate its application/domain envelope. */
export async function decodeHttpIpcResult<T>(
  response: Response,
  decodeData: IpcDataDecoder<T>
): Promise<IpcResult<T>> {
  try {
    const value: unknown = await response.json()
    return decodeIpcResult(value, decodeData)
  } catch {
    return networkError()
  }
}

/** Fetch and decode one exact IpcResult independently of the HTTP status code. */
export async function requestHttpIpcResult<T>(
  input: RequestInfo | URL,
  init: RequestInit | undefined,
  decodeData: IpcDataDecoder<T>
): Promise<IpcResult<T>> {
  try {
    return await decodeHttpIpcResult(await fetch(input, init), decodeData)
  } catch {
    return networkError()
  }
}
