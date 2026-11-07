import { decodeIpcResult, type IpcResult } from '@shared/types/ipc.types'
import type {
  McpHostAccessPolicy,
  McpHostAccessView,
  McpHostCall
} from '@shared/types/mcp-host.types'
import { invoke } from '@tauri-apps/api/core'
import { logFrontendError } from './log-api'
import { remoteAccessHeaders } from './remote-access-credential'
import { isTauriContext } from './tauri-runtime'

/** No transport may throw into a component or log arguments/backend error text. */
async function request<T>(
  command: string,
  path: string,
  method: string,
  body?: object
): Promise<IpcResult<T>> {
  try {
    if (isTauriContext()) {
      return decodeIpcResult(
        await invoke(command, body ? { ...body } : undefined),
        (data) => data as T
      )
    }
    const response = await fetch(`${window.location.origin}${path}`, {
      method,
      headers: remoteAccessHeaders({ 'content-type': 'application/json' }),
      ...(body ? { body: JSON.stringify(body) } : {})
    })
    if (response.status === 401)
      return { success: false, code: 'UNAUTHORIZED', error: 'Authentication required' }
    if (response.status === 403)
      return { success: false, code: 'FORBIDDEN', error: 'Host access management is unavailable' }
    if (response.status === 409)
      return {
        success: false,
        code: 'REVISION_CONFLICT',
        error: 'Reload the current policy before saving'
      }
    if (!response.ok)
      return { success: false, code: 'NETWORK_ERROR', error: 'Host access request failed' }
    return decodeIpcResult(await response.json(), (data) => data as T)
  } catch {
    void logFrontendError({ source: 'mcp-host-api', message: 'Host access transport failed' })
    return { success: false, code: 'NETWORK_ERROR', error: 'Host access request failed' }
  }
}

export function getMcpHostAccess(): Promise<IpcResult<McpHostAccessView>> {
  return request('mcp_host_access_get', '/mcp/host/access', 'GET')
}
export function putMcpHostAccess(
  expectedRevision: number,
  policy: McpHostAccessPolicy
): Promise<IpcResult<McpHostAccessPolicy>> {
  return request('mcp_host_access_put', '/mcp/host/access', 'PUT', { expectedRevision, policy })
}
export function callMcpHost(call: McpHostCall): Promise<IpcResult<unknown>> {
  return request('mcp_host_call', '/mcp/host/call', 'POST', call)
}
