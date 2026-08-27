import {
  type ConversationAggregateMutationOutcome,
  type ConversationId,
  type ConversationRecordV2,
  type ExecutionTarget,
  isConversationId,
  type ProjectAttachment,
  parseConversationAggregateMutationOutcome,
  parseConversationRecordV2,
  parseExecutionTarget,
  parseProjectAttachment
} from '@shared/types/conversation.types'
import {
  type ConversationApi,
  type ConversationBindingSnapshot,
  type ConversationHostStatus,
  type ConversationOpenOutcome,
  type LegacyConversationKey,
  type LegacyConversationResolution,
  parseConversationBindingSnapshot,
  parseConversationHostStatus,
  parseConversationOpenOutcome,
  parseConversationRecordV2Array,
  parseLegacyConversationResolution
} from '@shared/types/conversation-api.types'
import type { IpcDataDecoder, IpcResult } from '@shared/types/ipc.types'
import { requestHttpIpcResult } from '@/lib/http-ipc-result'
import { remoteAccessHeaders } from './acp-transport'

function serverBase(): string {
  return typeof window === 'undefined' ? '' : window.location.origin
}

function failure(code: string, error: string): IpcResult<never> {
  return { success: false, code, error }
}

function invalidConversationId(): IpcResult<never> {
  return failure(
    'CONVERSATION_INVALID_ID',
    'conversationId must be a canonical lowercase-hyphenated UUID'
  )
}

function requestJson<T>(
  path: string,
  decodeData: IpcDataDecoder<T>,
  init: RequestInit = { method: 'GET' }
): Promise<IpcResult<T>> {
  return requestHttpIpcResult(
    `${serverBase()}${path}`,
    {
      ...init,
      headers: remoteAccessHeaders(init.headers)
    },
    decodeData
  )
}

function postJson<T>(
  path: string,
  body: unknown,
  decodeData: IpcDataDecoder<T>
): Promise<IpcResult<T>> {
  return requestJson(path, decodeData, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  })
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
    return Promise.resolve(
      failure('VALIDATION_ERROR', 'expectedRevision must be a non-negative safe integer')
    )
  }
  return withConversationId(conversationId, operation)
}

export function createWebConversationApi(): ConversationApi {
  return {
    getHostStatus: () =>
      requestJson<ConversationHostStatus>(
        '/conversations/host-status',
        parseConversationHostStatus
      ),
    listConversations: () =>
      requestJson<ConversationRecordV2[]>('/conversations', parseConversationRecordV2Array),
    getConversation: (conversationId) =>
      withConversationId(conversationId, () =>
        requestJson<ConversationRecordV2>(
          `/conversations/${encodeURIComponent(conversationId)}`,
          parseConversationRecordV2
        )
      ),
    getCurrentBinding: (conversationId) =>
      withConversationId(conversationId, () =>
        requestJson<ConversationBindingSnapshot>(
          `/conversations/${encodeURIComponent(conversationId)}/binding`,
          parseConversationBindingSnapshot
        )
      ),
    openConversation: (conversationId) =>
      withConversationId(conversationId, () =>
        postJson<ConversationOpenOutcome>(
          `/conversations/${encodeURIComponent(conversationId)}/open`,
          {},
          parseConversationOpenOutcome
        )
      ),
    renameConversation: (conversationId, title) =>
      withConversationId(conversationId, () =>
        postJson<ConversationRecordV2>(
          `/conversations/${encodeURIComponent(conversationId)}/rename`,
          { title },
          parseConversationRecordV2
        )
      ),
    resolveLegacyConversationId: (request: LegacyConversationKey) => {
      if (
        !request ||
        !['legacyStorageKey', 'legacyAgentSessionId', 'legacyChatHistoryId'].includes(
          request.sourceKind
        ) ||
        typeof request.value !== 'string' ||
        !request.value.trim()
      ) {
        return Promise.resolve(failure('VALIDATION_ERROR', 'legacy request is invalid'))
      }
      return postJson<LegacyConversationResolution>(
        '/conversations/resolve-legacy',
        request,
        parseLegacyConversationResolution
      )
    },
    attachProject(
      conversationId: ConversationId,
      expectedRevision: number,
      attachment: ProjectAttachment
    ) {
      try {
        parseProjectAttachment(attachment)
      } catch {
        return Promise.resolve(failure('VALIDATION_ERROR', 'project attachment is invalid'))
      }
      return withExpectedRevision(conversationId, expectedRevision, () =>
        postJson<ConversationAggregateMutationOutcome>(
          `/conversations/${encodeURIComponent(conversationId)}/attach-project`,
          { expectedRevision, attachment },
          parseConversationAggregateMutationOutcome
        )
      )
    },
    detachProject(conversationId: ConversationId, expectedRevision: number) {
      return withExpectedRevision(conversationId, expectedRevision, () =>
        postJson<ConversationAggregateMutationOutcome>(
          `/conversations/${encodeURIComponent(conversationId)}/detach-project`,
          { expectedRevision },
          parseConversationAggregateMutationOutcome
        )
      )
    },
    updateExecutionTarget(
      conversationId: ConversationId,
      expectedRevision: number,
      executionTarget: ExecutionTarget
    ) {
      try {
        parseExecutionTarget(executionTarget)
      } catch {
        return Promise.resolve(failure('VALIDATION_ERROR', 'execution target is invalid'))
      }
      return withExpectedRevision(conversationId, expectedRevision, () =>
        postJson<ConversationAggregateMutationOutcome>(
          `/conversations/${encodeURIComponent(conversationId)}/execution-target`,
          { expectedRevision, executionTarget },
          parseConversationAggregateMutationOutcome
        )
      )
    },
    subscribeHostStatus(listener) {
      if (typeof window === 'undefined') return () => undefined
      window.addEventListener('online', listener)
      window.addEventListener('visibilitychange', listener)
      window.addEventListener('conversation-host-status', listener)
      return () => {
        window.removeEventListener('online', listener)
        window.removeEventListener('visibilitychange', listener)
        window.removeEventListener('conversation-host-status', listener)
      }
    }
  }
}

/** Exact core singleton selected by the production Conversation facade on web. */
export const webConversationApi = createWebConversationApi()
