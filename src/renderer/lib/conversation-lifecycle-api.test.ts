import { parseConversationId } from '@shared/types/conversation.types'
import type { ConversationLifecycleAction } from '@shared/types/conversation-lifecycle.types'
import { afterEach, expect, it, vi } from 'vitest'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import {
  _resetAcpTransportForTests,
  _setAcpTransportForTests,
  type AcpTransport
} from './acp-transport'
import {
  ConversationLifecycleApiError,
  createConversationLifecycleApi
} from './conversation-lifecycle-api'
import { HTTP_IPC_NETWORK_ERROR_MESSAGE } from './http-ipc-result'

const conversationId = parseConversationId('018f7a1c-1b4d-7c8a-9f01-0123456789ab')

function response(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: 'status',
    json: vi.fn(async () => body)
  } as unknown as Response
}

function outcome(action: ConversationLifecycleAction, previousRevision: number) {
  const deleteAction = action === 'deleteConversation'
  return {
    status: 'updated' as const,
    action,
    conversationId,
    previousRevision,
    revision: deleteAction ? previousRevision : previousRevision + 1,
    workspaceCwd: '/visible/conversation',
    lifecycleState: deleteAction ? ('deleted' as const) : ('ready' as const),
    currentBinding: deleteAction
      ? null
      : {
          schemaVersion: 1 as const,
          bindingId: 'b2832b54-2ca4-4db4-93fd-f93bf6793114',
          agentSessionId: 'opaque/session',
          runtimeAgentId: 'agent-runtime',
          stableAgentNamespace: 'config:test',
          executionCwd: '/visible/conversation',
          boundAtUtc: '2026-08-15T09:45:16.000Z',
          state: 'active' as const
        },
    ...(action === 'replaceBinding' ? { previousAgentSessionId: 'opaque/previous' } : {})
  }
}

afterEach(() => {
  _resetAcpTransportForTests()
  vi.unstubAllGlobals()
})

it('decodes every lifecycle action through the sole HTTP IPC decoder', async () => {
  _setAcpTransportForTests({
    onEvent: vi.fn(() => vi.fn()),
    dispose: vi.fn()
  } as unknown as AcpTransport)
  const fetchMock = vi.fn()
  vi.stubGlobal('fetch', fetchMock)
  const api = createConversationLifecycleApi('web')
  const actions = [
    ['detachBinding', () => api.detachBinding(conversationId, 1)],
    ['rebindDetachedBinding', () => api.rebindDetachedBinding(conversationId, 2)],
    ['suspendBinding', () => api.suspendBinding(conversationId, 3)],
    [
      'replaceBinding',
      () =>
        api.replaceBinding(
          conversationId,
          { schemaVersion: 1, conversationId, executionTarget: { kind: 'workspace' } },
          4
        )
    ],
    ['deleteConversation', () => api.deleteConversation(conversationId, 5)]
  ] as const

  for (const [index, [action, invokeAction]] of actions.entries()) {
    const data = outcome(action, index + 1)
    fetchMock.mockResolvedValueOnce(
      response({ success: true, data }, [409, 422, 500, 200, 409][index])
    )
    await expect(invokeAction()).resolves.toEqual(data)
  }
  expect(fetchMock).toHaveBeenCalledTimes(5)
  expect(fetchMock.mock.calls.map(([input]) => String(input).split('/').at(-1))).toEqual([
    'detach',
    'rebind',
    'suspend',
    'replace',
    'delete'
  ])

  const applicationFailure = {
    success: false as const,
    code: 'CONVERSATION_CONFLICT',
    error: 'stale revision'
  }
  fetchMock.mockResolvedValueOnce(response(applicationFailure, 200))
  await expect(api.detachBinding(conversationId, 1)).rejects.toEqual(
    new ConversationLifecycleApiError('CONVERSATION_CONFLICT', 'stale revision')
  )

  const compensation = {
    conversationId,
    primaryCode: 'CONVERSATION_DURABILITY_FAILED',
    providerCloseCode: 'ACP_CLOSE_FAILED',
    recoveryId: 'a'.repeat(64)
  }
  fetchMock.mockResolvedValueOnce(
    response(
      {
        success: false,
        code: 'ACP_COMPENSATION_FAILED',
        error: JSON.stringify(compensation)
      },
      500
    )
  )
  const compensationError = await api
    .replaceBinding(
      conversationId,
      { schemaVersion: 1, conversationId, executionTarget: { kind: 'workspace' } },
      4
    )
    .catch((error: unknown) => error)
  expect(compensationError).toBeInstanceOf(ConversationLifecycleApiError)
  expect(compensationError).toMatchObject({
    code: 'ACP_COMPENSATION_FAILED',
    compensation
  })

  for (const body of [
    { success: true },
    { success: true, data: { ...outcome('detachBinding', 1), extra: true } },
    { success: true, data: { ...outcome('deleteConversation', 5), lifecycleState: 'ready' } },
    { success: false, error: '', code: 'E' },
    { success: false, error: 'bad', code: 'E', data: null }
  ]) {
    fetchMock.mockResolvedValueOnce(response(body, 422))
    await expect(api.detachBinding(conversationId, 1)).rejects.toMatchObject({
      name: 'ConversationLifecycleApiError',
      code: 'NETWORK_ERROR',
      message: HTTP_IPC_NETWORK_ERROR_MESSAGE,
      compensation: null
    })
  }

  fetchMock.mockResolvedValueOnce({
    ok: true,
    status: 200,
    json: vi.fn(async () => {
      throw new SyntaxError('invalid body with /secret/path')
    })
  } as unknown as Response)
  await expect(api.detachBinding(conversationId, 1)).rejects.toMatchObject({
    code: 'NETWORK_ERROR',
    message: HTTP_IPC_NETWORK_ERROR_MESSAGE
  })

  fetchMock.mockRejectedValueOnce(new Error('offline token=hidden'))
  await expect(api.detachBinding(conversationId, 1)).rejects.toMatchObject({
    code: 'NETWORK_ERROR',
    message: HTTP_IPC_NETWORK_ERROR_MESSAGE
  })

  fetchMock.mockClear()
  await expect(api.detachBinding(conversationId, -1)).rejects.toMatchObject({
    code: 'VALIDATION_ERROR'
  })
  await expect(
    api.replaceBinding(
      conversationId,
      {
        schemaVersion: 1,
        conversationId,
        executionTarget: { kind: 'workspace' },
        extra: true
      } as never,
      1
    )
  ).rejects.toMatchObject({ code: 'VALIDATION_ERROR' })
  expect(fetchMock).not.toHaveBeenCalled()
})

it('normalizes Tauri decoder TypeErrors to a stable application error', async () => {
  invokeMock.mockRejectedValueOnce(new TypeError('IPC result must be an object'))
  const api = createConversationLifecycleApi('tauri')
  await expect(api.detachBinding(conversationId, 1)).rejects.toMatchObject({
    name: 'ConversationLifecycleApiError',
    code: 'NETWORK_ERROR',
    message: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    compensation: null
  })
})
