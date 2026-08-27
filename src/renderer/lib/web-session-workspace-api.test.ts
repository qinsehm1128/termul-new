import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { fetchMock, runtimeMock } = vi.hoisted(() => ({
  fetchMock: vi.fn(),
  runtimeMock: vi.fn(() => false)
}))

vi.mock('./tauri-runtime', () => ({ isTauriContext: runtimeMock }))

import type { SessionWorkspaceV1 } from '@shared/types/session-workspace.types'
import { HTTP_IPC_NETWORK_ERROR_MESSAGE } from './http-ipc-result'
import { webSessionWorkspaceApi } from './web-session-workspace-api'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const workspace: SessionWorkspaceV1 = {
  schemaVersion: 1,
  conversationId,
  revision: 0,
  updatedAtUtc: '',
  resources: [],
  projectionState: { status: 'native' }
}

function response(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    json: () => Promise.resolve(body)
  } as Response
}

describe('webSessionWorkspaceApi', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    runtimeMock.mockReturnValue(false)
    vi.stubGlobal('fetch', fetchMock)
  })

  afterEach(() => vi.unstubAllGlobals())

  it('uses encoded Conversation routes and the exact write body', async () => {
    fetchMock
      .mockResolvedValueOnce(
        response({ success: true, data: { status: 'missing', conversationId } })
      )
      .mockResolvedValueOnce(
        response({
          success: true,
          data: { status: 'updated', revision: 1, updatedAtUtc: '2026-08-15T10:00:00.000Z' }
        })
      )
    await webSessionWorkspaceApi.getWorkspace(conversationId)
    await webSessionWorkspaceApi.writeWorkspace(conversationId, null, workspace)
    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      `${window.location.origin}/conversations/${conversationId}/workspace`,
      { method: 'GET', headers: new Headers() }
    )
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      `${window.location.origin}/conversations/${conversationId}/workspace`,
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ basedRevision: null, workspace })
      })
    )
  })

  it('posts exact shared recovery requests and preserves stable application errors', async () => {
    const request = {
      recoveryId: 'a'.repeat(64),
      expectedRevision: 3,
      idempotencyKey: 'd70c2b93-71bc-4df0-85a5-15bd1b7cf452',
      action: 'startEmptyWorkspace' as const,
      payload: { conversationId, expectedWorkspaceRevision: null }
    }
    fetchMock.mockResolvedValueOnce(
      response({
        success: true,
        data: {
          recoveryId: request.recoveryId,
          action: request.action,
          authorization: 'mutation',
          status: 'resolvedStartedEmpty',
          recoveryRevision: 4,
          workspaceRevision: 1,
          workspaceChanged: true,
          sourcePaths: ['legacy.json'],
          sourceSha256: ['e'.repeat(64)],
          candidateFacts: [],
          provenance: []
        }
      })
    )
    await webSessionWorkspaceApi.resolveRecovery(request)
    expect(fetchMock).toHaveBeenCalledWith(
      `${window.location.origin}/conversation-recovery/resolve`,
      expect.objectContaining({ method: 'POST', body: JSON.stringify(request) })
    )

    fetchMock.mockResolvedValueOnce(
      response({ success: false, error: 'localhost only', code: 'FORBIDDEN' }, 403)
    )
    expect(await webSessionWorkspaceApi.writeWorkspace(conversationId, null, workspace)).toEqual({
      success: false,
      error: 'localhost only',
      code: 'FORBIDDEN'
    })
  })

  it('preserves typed success envelopes for HTTP 409 Conflict and 422 RecoveryRequired', async () => {
    const conflict = {
      status: 'conflict' as const,
      currentRevision: 6,
      currentUpdatedAtUtc: '2026-08-15T10:00:00.000Z',
      currentUpdateIdentity: 'browser-b'
    }
    const recoveryRequired = {
      status: 'recoveryRequired' as const,
      recoveryItems: [
        {
          recoveryId: 'a'.repeat(64),
          kind: 'ambiguous_workspace_manifest' as const,
          severity: 'warning' as const,
          sourcePaths: [],
          conversationIds: [conversationId],
          sourceSha256: [],
          candidateFacts: [],
          provenance: [],
          status: 'unresolved' as const,
          suggestedActions: ['inspect'] as const,
          revision: 7,
          associationDecisions: []
        }
      ]
    }
    fetchMock
      .mockResolvedValueOnce(response({ success: true, data: conflict }, 409))
      .mockResolvedValueOnce(response({ success: true, data: recoveryRequired }, 422))

    expect(await webSessionWorkspaceApi.writeWorkspace(conversationId, 4, workspace)).toEqual({
      success: true,
      data: conflict
    })
    expect(await webSessionWorkspaceApi.writeWorkspace(conversationId, 4, workspace)).toEqual({
      success: true,
      data: recoveryRequired
    })
  })

  it('maps rejected fetch, malformed envelopes, and JSON failures to NETWORK_ERROR', async () => {
    fetchMock.mockRejectedValueOnce(new Error('offline'))
    expect(await webSessionWorkspaceApi.getWorkspace(conversationId)).toMatchObject({
      success: false,
      code: 'NETWORK_ERROR'
    })
    fetchMock.mockResolvedValueOnce(response({}, 500))
    expect(await webSessionWorkspaceApi.getWorkspace(conversationId)).toEqual({
      success: false,
      error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
      code: 'NETWORK_ERROR'
    })
    fetchMock.mockResolvedValueOnce(response({ success: true }, 409))
    expect(await webSessionWorkspaceApi.getWorkspace(conversationId)).toEqual({
      success: false,
      error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
      code: 'NETWORK_ERROR'
    })
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      statusText: 'OK',
      json: () => Promise.reject(new Error('bad json'))
    } as Response)
    expect(await webSessionWorkspaceApi.getWorkspace(conversationId)).toMatchObject({
      success: false,
      code: 'NETWORK_ERROR'
    })
  })

  it('rejects malformed exact envelopes and wrong workspace domain tags consistently', async () => {
    const malformed = [
      { success: true, data: { status: 'updated', revision: 1, updatedAtUtc: 'x' } },
      { success: true, data: { status: 'missing', conversationId, extra: true } },
      { success: true, data: { status: 'loaded', workspace: { ...workspace, claim: 'secret' } } },
      { success: true, data: { status: 'recoveryRequired', recoveryItems: [{}] } },
      { success: false, error: 'bad', code: 'E', extra: true }
    ]
    for (const body of malformed) {
      fetchMock.mockResolvedValueOnce(response(body, 422))
      await expect(webSessionWorkspaceApi.getWorkspace(conversationId)).resolves.toEqual({
        success: false,
        error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
        code: 'NETWORK_ERROR'
      })
    }

    fetchMock.mockResolvedValueOnce(
      response(
        {
          success: true,
          data: { status: 'conflict', currentRevision: 0, currentUpdatedAtUtc: '' }
        },
        409
      )
    )
    await expect(
      webSessionWorkspaceApi.writeWorkspace(conversationId, 0, workspace)
    ).resolves.toEqual({
      success: false,
      error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
      code: 'NETWORK_ERROR'
    })
  })

  it('rejects invalid ConversationId and invalid recovery action without fetch', async () => {
    expect(await webSessionWorkspaceApi.getWorkspace('project-one')).toMatchObject({
      success: false,
      code: 'CONVERSATION_INVALID_ID'
    })
    expect(
      await webSessionWorkspaceApi.resolveRecovery({
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        action: 'associateConversation',
        idempotencyKey: 'bad',
        payload: { conversationId }
      })
    ).toMatchObject({ success: false, code: 'VALIDATION_ERROR' })
    expect(fetchMock).not.toHaveBeenCalled()
  })
})
