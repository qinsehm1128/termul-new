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
  if (!result.success) throw new AgentChatError(result.code ?? AGENT_CHAT_ERROR, result.error)
  return result.data
}

/** Stable codes shared with the desktop command and the se-server facade. */
export const AGENT_CHAT_NO_HOOK = 'AGENT_CHAT_NO_HOOK'
export const UNSUPPORTED_AGENT_CHAT = 'UNSUPPORTED_AGENT_CHAT'
export const AGENT_CHAT_ERROR = 'AGENT_CHAT_ERROR'

/** Absence is a state, not a fault; the terminal itself stays usable. */
export type AgentChatAbsence = 'no-hook' | 'unsupported'

const absenceMessages: Record<string, string> = {
  [AGENT_CHAT_NO_HOOK]: 'No agent is reporting a chat session for this terminal.',
  [UNSUPPORTED_AGENT_CHAT]: 'This agent session has no chat transcript view.'
}
const genericMessage = 'Agent chat is temporarily unavailable; the terminal is still usable.'

/** Transport failure that always carries a machine-readable code. */
export class AgentChatError extends Error {
  readonly code: string

  constructor(code: string, message = absenceMessages[code] ?? genericMessage) {
    super(message)
    this.name = 'AgentChatError'
    this.code = code
  }

  /** `String(error)` and templates render the message, never "Error: ...". */
  override toString(): string {
    return this.message
  }
}

/** Code for either transport: Tauri rejects with the bare code, web with a code field. */
export function agentChatErrorCode(cause: unknown): string {
  if (typeof cause === 'string') return cause
  if (cause && typeof cause === 'object' && 'code' in cause) {
    const code = (cause as { code?: unknown }).code
    if (typeof code === 'string' && code) return code
  }
  return AGENT_CHAT_ERROR
}

/** Expected absence, or null for a real failure. */
export function agentChatAbsence(cause: unknown): AgentChatAbsence | null {
  const code = agentChatErrorCode(cause)
  if (code === AGENT_CHAT_NO_HOOK) return 'no-hook'
  if (code === UNSUPPORTED_AGENT_CHAT) return 'unsupported'
  return null
}

/** Normalize both transports so callers branch on one error type. */
function failure(cause: unknown): AgentChatError {
  if (cause instanceof AgentChatError) return cause
  const code = agentChatErrorCode(cause)
  if (absenceMessages[code]) return new AgentChatError(code)
  const message = cause instanceof Error ? cause.message : undefined
  return new AgentChatError(code, message || genericMessage)
}

export function createTauriAgentChatApi(): AgentChatApi<ChatMessage, ToolCall> {
  return {
    snapshot: async (terminalId, after, before) => {
      try {
        return await invoke<AgentChatSnapshot<ChatMessage, ToolCall>>('agent_chat_snapshot', {
          terminalId,
          after: after ?? null,
          before: before ?? null
        })
      } catch (cause) {
        throw failure(cause)
      }
    },
    send: async (terminalId, text, options) => {
      try {
        return await invoke('agent_chat_send', {
          terminalId,
          text,
          ...(options?.force !== undefined ? { force: options.force } : {}),
          ...(options?.delivery_id !== undefined ? { deliveryId: options.delivery_id } : {})
        })
      } catch (cause) {
        throw failure(cause)
      }
    },
    control: async (terminalId, action) => {
      try {
        return await invoke('agent_chat_control', { terminalId, action })
      } catch (cause) {
        throw failure(cause)
      }
    }
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
  send: async (terminalId, text, options) => {
    if (!options) return unwrap(await webServerAgentChat.send(terminalId, text))
    const headers = new Headers(remoteAccessHeaders())
    headers.set('Content-Type', 'application/json')
    const response = await fetch(
      `${window.location.origin}/agent-chat/${encodeURIComponent(terminalId)}/send`,
      {
        method: 'POST',
        credentials: 'same-origin',
        headers,
        body: JSON.stringify({ text, ...options })
      }
    )
    const body = await response.json()
    if (!response.ok)
      throw Object.assign(
        new Error(body.error ?? `HTTP ${response.status} ${response.statusText}`),
        { code: body.code ?? 'NETWORK_ERROR' }
      )
    return unwrap(body as IpcResult<unknown>)
  },
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
  send: (terminalId, text, options) =>
    (isTauriContext() ? desktop : webAgentChatApi).send(terminalId, text, options),
  control: (terminalId, action) =>
    (isTauriContext() ? desktop : webAgentChatApi).control(terminalId, action)
}
