import { type ConversationId, isConversationId } from '@shared/types/conversation.types'
import type {
  RecoveryActionResult,
  ResolveRecoveryItemRequest
} from '@shared/types/conversation-recovery.types'
import {
  parseRecoveryActionResult,
  parseResolveRecoveryItemRequest
} from '@shared/types/conversation-recovery.types'
import type { IpcDataDecoder, IpcResult } from '@shared/types/ipc.types'
import {
  parseSessionWorkspaceLoadOutcome,
  parseSessionWorkspaceWriteOutcome,
  type SessionWorkspaceApi,
  type SessionWorkspaceLoadOutcome,
  type SessionWorkspaceV1,
  type SessionWorkspaceWriteOutcome
} from '@shared/types/session-workspace.types'
import type { InvokeArgs } from '@tauri-apps/api/core'
import { invokeDecodedIpcResult } from './invoke-decoded-ipc-result'
import { isTauriContext } from './tauri-runtime'

function invokeIpc<T>(
  command: string,
  decodeData: IpcDataDecoder<T>,
  args?: InvokeArgs
): Promise<IpcResult<T>> {
  return invokeDecodedIpcResult(command, decodeData, args)
}

function invalidConversationId(): IpcResult<never> {
  return {
    success: false,
    error: 'conversationId must be a canonical lowercase-hyphenated UUID',
    code: 'CONVERSATION_INVALID_ID'
  }
}

export function createTauriSessionWorkspaceApi(): SessionWorkspaceApi {
  return {
    async getWorkspace(
      conversationId: ConversationId
    ): Promise<IpcResult<SessionWorkspaceLoadOutcome>> {
      if (!isConversationId(conversationId)) return invalidConversationId()
      if (!isTauriContext()) {
        return {
          success: false,
          error: 'session_workspace_get requires the Tauri runtime',
          code: 'INVOKE_ERROR'
        }
      }
      return invokeIpc('session_workspace_get', parseSessionWorkspaceLoadOutcome, {
        conversationId
      })
    },

    async writeWorkspace(
      conversationId: ConversationId,
      basedRevision: number | null,
      workspace: SessionWorkspaceV1
    ): Promise<IpcResult<SessionWorkspaceWriteOutcome>> {
      if (!isConversationId(conversationId) || workspace.conversationId !== conversationId) {
        return invalidConversationId()
      }
      if (!isTauriContext()) {
        return {
          success: false,
          error: 'session_workspace_write requires the Tauri runtime',
          code: 'INVOKE_ERROR'
        }
      }
      return invokeIpc('session_workspace_write', parseSessionWorkspaceWriteOutcome, {
        conversationId,
        basedRevision,
        workspace
      })
    },

    async resolveRecovery(
      request: ResolveRecoveryItemRequest
    ): Promise<IpcResult<RecoveryActionResult>> {
      let parsed: ResolveRecoveryItemRequest
      try {
        parsed = parseResolveRecoveryItemRequest(request)
      } catch (error) {
        return {
          success: false,
          error: error instanceof Error ? error.message : String(error),
          code: 'VALIDATION_ERROR'
        }
      }
      if (!isTauriContext()) {
        return {
          success: false,
          error: 'conversation_recovery_resolve requires the Tauri runtime',
          code: 'INVOKE_ERROR'
        }
      }
      return invokeIpc('conversation_recovery_resolve', parseRecoveryActionResult, {
        request: parsed
      })
    }
  }
}

/** Exact Tauri workspace singleton selected by the production workspace facade. */
export const tauriSessionWorkspaceApi = createTauriSessionWorkspaceApi()
