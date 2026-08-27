import { type ConversationId, isConversationId } from '@shared/types/conversation.types'
import {
  parseRecoveryActionResult,
  parseResolveRecoveryItemRequest,
  type RecoveryActionResult,
  type ResolveRecoveryItemRequest
} from '@shared/types/conversation-recovery.types'
import type { IpcDataDecoder, IpcResult } from '@shared/types/ipc.types'
import {
  parseSessionWorkspaceLoadOutcome,
  parseSessionWorkspaceV1,
  parseSessionWorkspaceWriteOutcome,
  type SessionWorkspaceApi,
  type SessionWorkspaceLoadOutcome,
  type SessionWorkspaceV1,
  type SessionWorkspaceWriteOutcome,
  type SessionWorkspaceWriteRequestBody
} from '@shared/types/session-workspace.types'
import { requestHttpIpcResult } from '@/lib/http-ipc-result'
import { remoteAccessHeaders } from './acp-transport'
import { isTauriContext } from './tauri-runtime'

function serverBase(): string {
  if (isTauriContext()) return ''
  if (typeof window === 'undefined' || !window.location) return ''
  return window.location.origin
}

function invalidConversationId(): IpcResult<never> {
  return {
    success: false,
    error: 'conversationId must be a canonical lowercase-hyphenated UUID',
    code: 'CONVERSATION_INVALID_ID'
  }
}

function getJson<T>(path: string, decodeData: IpcDataDecoder<T>): Promise<IpcResult<T>> {
  return requestHttpIpcResult(
    `${serverBase()}${path}`,
    {
      method: 'GET',
      headers: remoteAccessHeaders()
    },
    decodeData
  )
}

function postJson<T>(
  path: string,
  body: unknown,
  decodeData: IpcDataDecoder<T>
): Promise<IpcResult<T>> {
  return requestHttpIpcResult(
    `${serverBase()}${path}`,
    {
      method: 'POST',
      headers: remoteAccessHeaders({ 'content-type': 'application/json' }),
      body: JSON.stringify(body)
    },
    decodeData
  )
}

export const webSessionWorkspaceApi: SessionWorkspaceApi = {
  getWorkspace(conversationId: ConversationId): Promise<IpcResult<SessionWorkspaceLoadOutcome>> {
    if (!isConversationId(conversationId)) return Promise.resolve(invalidConversationId())
    return getJson(
      `/conversations/${encodeURIComponent(conversationId)}/workspace`,
      parseSessionWorkspaceLoadOutcome
    )
  },

  writeWorkspace(
    conversationId: ConversationId,
    basedRevision: number | null,
    workspace: SessionWorkspaceV1
  ): Promise<IpcResult<SessionWorkspaceWriteOutcome>> {
    if (!isConversationId(conversationId) || workspace.conversationId !== conversationId) {
      return Promise.resolve(invalidConversationId())
    }
    if (basedRevision !== null && (!Number.isSafeInteger(basedRevision) || basedRevision < 0)) {
      return Promise.resolve({
        success: false,
        error: 'basedRevision must be null or a non-negative safe integer',
        code: 'VALIDATION_ERROR'
      })
    }
    try {
      parseSessionWorkspaceV1(workspace)
    } catch {
      return Promise.resolve({
        success: false,
        error: 'workspace is invalid',
        code: 'VALIDATION_ERROR'
      })
    }
    const body: SessionWorkspaceWriteRequestBody = { basedRevision, workspace }
    return postJson(
      `/conversations/${encodeURIComponent(conversationId)}/workspace`,
      body,
      parseSessionWorkspaceWriteOutcome
    )
  },

  resolveRecovery(request: ResolveRecoveryItemRequest): Promise<IpcResult<RecoveryActionResult>> {
    let parsed: ResolveRecoveryItemRequest
    try {
      parsed = parseResolveRecoveryItemRequest(request)
    } catch (error) {
      return Promise.resolve({
        success: false,
        error: error instanceof Error ? error.message : String(error),
        code: 'VALIDATION_ERROR'
      })
    }
    return postJson('/conversation-recovery/resolve', parsed, parseRecoveryActionResult)
  }
}
