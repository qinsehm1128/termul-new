import { decodeIpcResult, type IpcDataDecoder, type IpcResult } from '@shared/types/ipc.types'
import { type InvokeArgs, invoke } from '@tauri-apps/api/core'
import { HTTP_IPC_NETWORK_ERROR_MESSAGE } from '@/lib/http-ipc-result'

function decoderFailure(): IpcResult<never> {
  return {
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  }
}

/** Normalize decoder TypeErrors to the same application error HTTP uses. */
export function normalizeDecodedIpcFailure(error: unknown): IpcResult<never> {
  if (error instanceof TypeError) return decoderFailure()
  if (error && typeof error === 'object') {
    const record = error as Record<string, unknown>
    if (typeof record.code === 'string') {
      return {
        success: false,
        code: record.code,
        error:
          typeof record.error === 'string'
            ? record.error
            : typeof record.message === 'string'
              ? record.message
              : record.code
      }
    }
  }
  return {
    success: false,
    code: 'INVOKE_ERROR',
    error: error instanceof Error ? error.message : String(error)
  }
}

/**
 * Invoke a Tauri command and decode the exact IpcResult envelope with the same
 * domain parsers used by the HTTP facades. Decoder TypeErrors never leak.
 */
export async function invokeDecodedIpcResult<T>(
  command: string,
  decodeData: IpcDataDecoder<T>,
  args?: InvokeArgs
): Promise<IpcResult<T>> {
  try {
    return decodeIpcResult(await invoke<unknown>(command, args), decodeData)
  } catch (error) {
    return normalizeDecodedIpcFailure(error)
  }
}
