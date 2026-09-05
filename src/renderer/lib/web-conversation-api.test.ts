import type {
  ConversationAggregateMutationAction,
  ConversationAggregateMutationOutcome,
  ConversationId,
  ExecutionTarget,
  ProjectAttachment
} from '@shared/types/conversation.types'
import { parseConversationId } from '@shared/types/conversation.types'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { HTTP_IPC_NETWORK_ERROR_MESSAGE } from './http-ipc-result'
import { createWebConversationApi } from './web-conversation-api'

const conversationId = parseConversationId('018f7a1c-1b4d-7c8a-9f01-0123456789ab')

function response(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: 'status',
    json: vi.fn(async () => body)
  } as unknown as Response
}

function record(
  lastSeq = 4,
  attachment: ProjectAttachment | null = null,
  target: ExecutionTarget = { kind: 'workspace' }
) {
  return {
    schemaVersion: 2 as const,
    conversationId,
    createdAtUtc: '2026-08-15T09:45:15.123Z',
    creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
    workspaceCwd: '/visible/conversation',
    executionTarget: target,
    projectAttachment: attachment,
    lifecycleState: 'ready' as const,
    lastSeq,
    createdBy: 'se-manager' as const
  }
}

const attachment: ProjectAttachment = {
  schemaVersion: 1,
  projectId: 'project-1',
  attachedAtUtc: '2026-08-15T10:00:00.000Z',
  projectPathSnapshot: '/projects/se',
  worktreePath: null,
  worktreeBranch: null
}

function aggregate(
  action: ConversationAggregateMutationAction,
  previousRevision: number,
  projectAttachment: ProjectAttachment | null,
  executionTarget: ExecutionTarget
): ConversationAggregateMutationOutcome {
  const identity = {
    conversationId,
    createdAtUtc: '2026-08-15T09:45:15.123Z',
    creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
    workspaceCwd: '/visible/conversation'
  }
  return {
    status: 'updated',
    action,
    conversationId,
    previousRevision,
    revision: previousRevision + 1,
    identityBefore: identity,
    identityAfter: { ...identity },
    projectAttachment,
    executionTarget,
    conversation: record(previousRevision + 1, projectAttachment, executionTarget)
  }
}

beforeEach(() => vi.stubGlobal('fetch', vi.fn()))
afterEach(() => vi.unstubAllGlobals())

it('decodes list get open legacy and mutation endpoints through the sole HTTP decoder', async () => {
  const fetchMock = vi.mocked(fetch)
  const api = createWebConversationApi()
  const hostStatus = {
    hostKind: 'standalone' as const,
    state: 'ready' as const,
    code: 'OK',
    migrationPhase: 'finalized' as const,
    readerPrecedence: 'conversationV2Only' as const,
    recoveryItemCount: 0,
    recoveryItems: []
  }
  const open = {
    conversation: record(),
    workspace: { status: 'missing' as const, conversationId }
  }
  const legacy = { conversationId, canonicalRoute: `#/c/${conversationId}` as const }
  const target: ExecutionTarget = {
    kind: 'project_root',
    projectId: attachment.projectId,
    projectRoot: attachment.projectPathSnapshot
  }
  const attached = aggregate('attachProject', 4, attachment, { kind: 'workspace' })
  const detached = aggregate('detachProject', 5, null, { kind: 'workspace' })
  const retargeted = aggregate('updateExecutionTarget', 6, attachment, target)

  fetchMock
    .mockResolvedValueOnce(response({ success: true, data: hostStatus }, 500))
    .mockResolvedValueOnce(response({ success: true, data: [record()] }, 409))
    .mockResolvedValueOnce(response({ success: true, data: record() }, 422))
    .mockResolvedValueOnce(response({ success: true, data: open }, 500))
    .mockResolvedValueOnce(response({ success: true, data: legacy }, 409))
    .mockResolvedValueOnce(response({ success: true, data: attached }, 422))
    .mockResolvedValueOnce(response({ success: true, data: detached }, 500))
    .mockResolvedValueOnce(response({ success: true, data: retargeted }, 409))

  await expect(api.getHostStatus()).resolves.toEqual({ success: true, data: hostStatus })
  await expect(api.listConversations()).resolves.toEqual({ success: true, data: [record()] })
  await expect(api.getConversation(conversationId)).resolves.toEqual({
    success: true,
    data: record()
  })
  await expect(api.openConversation(conversationId)).resolves.toEqual({ success: true, data: open })
  await expect(
    api.resolveLegacyConversationId({ sourceKind: 'legacyStorageKey', value: 'legacy-one' })
  ).resolves.toEqual({ success: true, data: legacy })
  await expect(api.attachProject(conversationId, 4, attachment)).resolves.toEqual({
    success: true,
    data: attached
  })
  await expect(api.detachProject(conversationId, 5)).resolves.toEqual({
    success: true,
    data: detached
  })
  await expect(api.updateExecutionTarget(conversationId, 6, target)).resolves.toEqual({
    success: true,
    data: retargeted
  })

  const stableFailure = { success: false as const, error: 'stable failure', code: 'FORBIDDEN' }
  fetchMock.mockResolvedValueOnce(response(stableFailure, 200))
  await expect(api.listConversations()).resolves.toBe(stableFailure)
  fetchMock.mockResolvedValueOnce(response(stableFailure, 500))
  await expect(api.listConversations()).resolves.toBe(stableFailure)

  for (const body of [
    { success: true, data: [{}] },
    { success: true, data: { ...record(), extra: true } },
    { success: true, data: { conversation: record(), workspace: { status: 'wrong' } } },
    { success: true, data: legacy, extra: true },
    { success: false, error: '', code: 'E' }
  ]) {
    fetchMock.mockResolvedValueOnce(response(body, 422))
    await expect(api.listConversations()).resolves.toEqual({
      success: false,
      error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
      code: 'NETWORK_ERROR'
    })
  }
  fetchMock.mockResolvedValueOnce({
    ok: true,
    status: 200,
    json: vi.fn(async () => {
      throw new SyntaxError('invalid JSON with secret body')
    })
  } as unknown as Response)
  await expect(api.listConversations()).resolves.toEqual({
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  })
  fetchMock.mockRejectedValueOnce(new Error('/secret/path token=hidden'))
  await expect(api.listConversations()).resolves.toEqual({
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  })

  fetchMock.mockClear()
  const malformedId = 'not-a-conversation' as ConversationId
  await expect(api.getConversation(malformedId)).resolves.toMatchObject({
    success: false,
    code: 'CONVERSATION_INVALID_ID'
  })
  await expect(api.detachProject(conversationId, -1)).resolves.toMatchObject({
    success: false,
    code: 'VALIDATION_ERROR'
  })
  await expect(
    api.resolveLegacyConversationId({ sourceKind: 'legacyStorageKey', value: '   ' })
  ).resolves.toMatchObject({ success: false, code: 'VALIDATION_ERROR' })
  await expect(
    api.attachProject(conversationId, 4, { ...attachment, extra: true } as ProjectAttachment)
  ).resolves.toMatchObject({ success: false, code: 'VALIDATION_ERROR' })
  await expect(
    api.updateExecutionTarget(conversationId, 4, {
      kind: 'workspace',
      projectId: 'hidden'
    } as unknown as ExecutionTarget)
  ).resolves.toMatchObject({ success: false, code: 'VALIDATION_ERROR' })
  expect(fetchMock).not.toHaveBeenCalled()
})
