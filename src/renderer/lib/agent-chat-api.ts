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
import { isTauriContext } from './tauri-runtime'
import { webServerAgentChat } from './web-server-api'

function unwrap<T>(result: IpcResult<T>): T {
  if (!result.success) throw new Error(result.error)
  return result.data
}

export function createTauriAgentChatApi(): AgentChatApi<ChatMessage, ToolCall> {
  return {
    snapshot: (terminalId, after) =>
      invoke('agent_chat_snapshot', { terminalId, after: after ?? null }),
    send: (terminalId, text) => invoke('agent_chat_send', { terminalId, text }),
    control: (terminalId, action) => invoke('agent_chat_control', { terminalId, action })
  }
}

export const webAgentChatApi: AgentChatApi<ChatMessage, ToolCall> = {
  snapshot: async (terminalId, after) =>
    unwrap(await webServerAgentChat.snapshot(terminalId, after)) as AgentChatSnapshot<
      ChatMessage,
      ToolCall
    >,
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
  snapshot: (terminalId, after) =>
    (isTauriContext() ? desktop : webAgentChatApi).snapshot(terminalId, after),
  send: (terminalId, text) => (isTauriContext() ? desktop : webAgentChatApi).send(terminalId, text),
  control: (terminalId, action) =>
    (isTauriContext() ? desktop : webAgentChatApi).control(terminalId, action)
}
