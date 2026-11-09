import type {
  AgentChatApi,
  AgentChatSnapshot,
  AgentChatUpdated
} from '@shared/types/agent-chat.types'
import type { IpcResult } from '@shared/types/ipc.types'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { ChatMessage } from '../features/agent-session/stores/acp-store'
import type { ToolCall } from './acp-api'
import { remoteAccessHeaders } from './remote-access-credential'
import { isTauriContext } from './tauri-runtime'
import { webServerAgentChat } from './web-server-api'

function unwrap<T>(result: IpcResult<T>): T {
  if (!result.success) throw Object.assign(new Error(result.error), { code: result.code })
  return result.data
}

export function createTauriAgentChatApi(): AgentChatApi<ChatMessage, ToolCall> {
  return {
    snapshot: (terminalId, after, before) =>
      invoke('agent_chat_snapshot', { terminalId, after: after ?? null, before: before ?? null }),
    send: (terminalId, text) => invoke('agent_chat_send', { terminalId, text }),
    control: (terminalId, action) => invoke('agent_chat_control', { terminalId, action })
  }
}

export const webAgentChatApi: AgentChatApi<ChatMessage, ToolCall> = {
  snapshot: async (terminalId, after, before) => {
    const query = new URLSearchParams()
    if (after !== undefined) query.set('after', String(after))
    if (before !== undefined) query.set('before', String(before))
    const suffix = query.size ? `?${query}` : ''
    let result: IpcResult<AgentChatSnapshot<ChatMessage, ToolCall>>
    try {
      const response = await fetch(
        `${window.location.origin}/agent-chat/${encodeURIComponent(terminalId)}${suffix}`,
        {
          method: 'GET',
          credentials: 'same-origin',
          headers: remoteAccessHeaders()
        }
      )
      const body = await response.json()
      if (!response.ok) {
        const denied = [
          'FORBIDDEN',
          'DEVICE_REVOKED',
          'INVALID_CREDENTIAL',
          'RATE_LIMITED'
        ].includes(body.code)
        result = {
          success: false,
          code: denied ? body.code : 'NETWORK_ERROR',
          error: denied
            ? (body.error ?? (body.capability ? `Forbidden: ${body.capability}` : body.code))
            : `HTTP ${response.status} ${response.statusText}`
        }
      } else result = body as IpcResult<AgentChatSnapshot<ChatMessage, ToolCall>>
    } catch (cause) {
      result = {
        success: false,
        code: 'NETWORK_ERROR',
        error: cause instanceof Error ? cause.message : String(cause)
      }
    }
    return unwrap(result)
  },
  send: async (terminalId, text) => unwrap(await webServerAgentChat.send(terminalId, text)),
  control: async (terminalId, action) =>
    unwrap(await webServerAgentChat.control(terminalId, action))
}

export function subscribeAgentChatUpdated(
  callback: (metadata: AgentChatUpdated) => void
): Promise<() => void> {
  if (!isTauriContext()) return Promise.resolve(() => {})
  return listen<AgentChatUpdated>('agent-chat-updated', ({ payload }) => callback(payload))
}

const desktop = createTauriAgentChatApi()
/** Both App and TauriApp use this facade; credentials stay in the HTTP transport. */
export const agentChatApi: AgentChatApi<ChatMessage, ToolCall> = {
  snapshot: (terminalId, after, before) =>
    (isTauriContext() ? desktop : webAgentChatApi).snapshot(terminalId, after, before),
  send: (terminalId, text) => (isTauriContext() ? desktop : webAgentChatApi).send(terminalId, text),
  control: (terminalId, action) =>
    (isTauriContext() ? desktop : webAgentChatApi).control(terminalId, action)
}
