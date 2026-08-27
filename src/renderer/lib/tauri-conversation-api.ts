import {
  type ConversationAggregateMutationOutcome,
  type ConversationId,
  type ConversationRecordV2,
  type ExecutionTarget,
  isConversationId,
  type ProjectAttachment,
  parseConversationAggregateMutationOutcome,
  parseConversationRecordV2
} from '@shared/types/conversation.types'
import type {
  ConversationApi,
  ConversationHostStatus,
  ConversationOpenOutcome,
  LegacyConversationKey,
  LegacyConversationResolution
} from '@shared/types/conversation-api.types'
import {
  parseConversationBindingSnapshot,
  parseConversationHostStatus,
  parseConversationOpenOutcome,
  parseConversationRecordV2Array,
  parseLegacyConversationResolution
} from '@shared/types/conversation-api.types'
import type { IpcDataDecoder, IpcResult } from '@shared/types/ipc.types'
import { listen } from '@tauri-apps/api/event'
import { invokeDecodedIpcResult } from './invoke-decoded-ipc-result'

export function normalizeConversationError(error: unknown): IpcResult<never> {
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

function invalidConversationId(): IpcResult<never> {
  return {
    success: false,
    code: 'CONVERSATION_INVALID_ID',
    error: 'conversationId must be a canonical lowercase-hyphenated UUID'
  }
}

async function invokeConversation<T>(
  command: string,
  decodeData: IpcDataDecoder<T>,
  args?: Record<string, unknown>
): Promise<IpcResult<T>> {
  return invokeDecodedIpcResult(command, decodeData, args)
}

function withConversationId<T>(
  conversationId: ConversationId,
  operation: () => Promise<IpcResult<T>>
): Promise<IpcResult<T>> {
  return isConversationId(conversationId) ? operation() : Promise.resolve(invalidConversationId())
}

function withExpectedRevision<T>(
  conversationId: ConversationId,
  expectedRevision: number,
  operation: () => Promise<IpcResult<T>>
): Promise<IpcResult<T>> {
  if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 0) {
    return Promise.resolve({
      success: false,
      code: 'VALIDATION_ERROR',
      error: 'expectedRevision must be a non-negative safe integer'
    })
  }
  return withConversationId(conversationId, operation)
}

export function createTauriConversationApi(): ConversationApi {
  return {
    getHostStatus: () =>
      invokeConversation('conversation_host_status', parseConversationHostStatus),
    listConversations: () =>
      invokeConversation('conversation_list', parseConversationRecordV2Array),
    getConversation: (conversationId) =>
      withConversationId(conversationId, () =>
        invokeConversation('conversation_get', parseConversationRecordV2, { conversationId })
      ),
    getCurrentBinding: (conversationId) =>
      withConversationId(conversationId, () =>
        invokeConversation('conversation_get_binding', parseConversationBindingSnapshot, {
          conversationId
        })
      ),
    openConversation: (conversationId) =>
      withConversationId(conversationId, () =>
        invokeConversation('conversation_open', parseConversationOpenOutcome, { conversationId })
      ),
    renameConversation: (conversationId, title) =>
      withConversationId(conversationId, () =>
        invokeConversation('conversation_rename', parseConversationRecordV2, {
          conversationId,
          title
        })
      ),
    resolveLegacyConversationId: (request: LegacyConversationKey) => {
      if (!request.value.trim()) {
        return Promise.resolve({
          success: false,
          code: 'VALIDATION_ERROR',
          error: 'legacy value must be non-empty'
        })
      }
      return invokeConversation(
        'conversation_resolve_legacy_id',
        parseLegacyConversationResolution,
        { request }
      )
    },
    attachProject(
      conversationId: ConversationId,
      expectedRevision: number,
      attachment: ProjectAttachment
    ) {
      return withExpectedRevision(conversationId, expectedRevision, () =>
        invokeConversation(
          'conversation_attach_project',
          parseConversationAggregateMutationOutcome,
          {
            conversationId,
            expectedRevision,
            attachment
          }
        )
      )
    },
    detachProject(conversationId: ConversationId, expectedRevision: number) {
      return withExpectedRevision(conversationId, expectedRevision, () =>
        invokeConversation(
          'conversation_detach_project',
          parseConversationAggregateMutationOutcome,
          {
            conversationId,
            expectedRevision
          }
        )
      )
    },
    updateExecutionTarget(
      conversationId: ConversationId,
      expectedRevision: number,
      executionTarget: ExecutionTarget
    ) {
      return withExpectedRevision(conversationId, expectedRevision, () =>
        invokeConversation(
          'conversation_update_execution_target',
          parseConversationAggregateMutationOutcome,
          { conversationId, expectedRevision, executionTarget }
        )
      )
    },
    subscribeHostStatus(listener) {
      let active = true
      let unlisten: (() => void) | undefined
      void listen('conversation:host-status', listener).then((dispose) => {
        if (active) unlisten = dispose
        else dispose()
      })
      return () => {
        active = false
        unlisten?.()
      }
    }
  }
}

/** Exact core singleton selected by the production Conversation facade on Tauri. */
export const tauriConversationApi = createTauriConversationApi()
