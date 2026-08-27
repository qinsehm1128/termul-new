import { beforeEach, describe, expect, it, vi } from 'vitest'

const { invokeMock, runtimeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  runtimeMock: vi.fn(() => true)
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('./tauri-runtime', () => ({ isTauriContext: runtimeMock }))

import type { SessionWorkspaceV1 } from '@shared/types/session-workspace.types'
import { createTauriSessionWorkspaceApi } from './tauri-session-workspace-api'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
const workspace: SessionWorkspaceV1 = {
  schemaVersion: 1,
  conversationId,
  revision: 0,
  updatedAtUtc: '',
  updateIdentity: 'test',
  topology: null,
  activePaneId: null,
  resources: [],
  projectionState: { status: 'native' }
}

describe('createTauriSessionWorkspaceApi', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    runtimeMock.mockReturnValue(true)
  })

  it('uses canonical command names and request fields', async () => {
    invokeMock
      .mockResolvedValueOnce({ success: true, data: { status: 'missing', conversationId } })
      .mockResolvedValueOnce({
        success: true,
        data: { status: 'updated', revision: 1, updatedAtUtc: '2026-08-15T10:00:00.000Z' }
      })
      .mockResolvedValueOnce({
        success: true,
        data: {
          recoveryId: 'a'.repeat(64),
          action: 'inspect',
          authorization: 'read',
          status: 'unresolved',
          recoveryRevision: 1,
          workspaceRevision: null,
          workspaceChanged: false,
          sourcePaths: [],
          sourceSha256: [],
          candidateFacts: [],
          provenance: []
        }
      })
    const api = createTauriSessionWorkspaceApi()

    await api.getWorkspace(conversationId)
    await api.writeWorkspace(conversationId, null, workspace)
    await api.resolveRecovery({
      recoveryId: 'a'.repeat(64),
      expectedRevision: 1,
      action: 'inspect',
      payload: {}
    })

    expect(invokeMock).toHaveBeenNthCalledWith(1, 'session_workspace_get', { conversationId })
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'session_workspace_write', {
      conversationId,
      basedRevision: null,
      workspace
    })
    expect(invokeMock).toHaveBeenNthCalledWith(3, 'conversation_recovery_resolve', {
      request: {
        recoveryId: 'a'.repeat(64),
        expectedRevision: 1,
        action: 'inspect',
        payload: {}
      }
    })
  })

  it('rejects noncanonical or mismatched ConversationIds before invoke', async () => {
    const api = createTauriSessionWorkspaceApi()
    expect(await api.getWorkspace(conversationId.toUpperCase())).toMatchObject({
      success: false,
      code: 'CONVERSATION_INVALID_ID'
    })
    expect(
      await api.writeWorkspace('5f7a1c01-4d1b-4c8a-af01-0123456789ab', null, workspace)
    ).toMatchObject({ success: false, code: 'CONVERSATION_INVALID_ID' })
    expect(invokeMock).not.toHaveBeenCalled()
  })

  it('normalizes invoke failures and preserves conflict/recovery success variants', async () => {
    const api = createTauriSessionWorkspaceApi()
    invokeMock.mockRejectedValueOnce(new Error('bridge down'))
    expect(await api.getWorkspace(conversationId)).toEqual({
      success: false,
      error: 'bridge down',
      code: 'INVOKE_ERROR'
    })

    invokeMock.mockResolvedValueOnce({
      success: true,
      data: {
        status: 'conflict',
        currentRevision: 4,
        currentUpdatedAtUtc: '2026-08-15T10:00:00.000Z'
      }
    })
    expect(await api.writeWorkspace(conversationId, 2, workspace)).toMatchObject({
      success: true,
      data: { status: 'conflict', currentRevision: 4 }
    })
  })

  it('rejects malformed native success data with the same application error as HTTP', async () => {
    const api = createTauriSessionWorkspaceApi()
    invokeMock.mockResolvedValueOnce({ success: true, data: { extra: true } })
    await expect(api.getWorkspace(conversationId)).resolves.toEqual({
      success: false,
      error: 'Invalid response from host',
      code: 'NETWORK_ERROR'
    })
  })

  it('uses the exact shared RecoveryAction parser', async () => {
    const api = createTauriSessionWorkspaceApi()
    const invalid = await api.resolveRecovery({
      recoveryId: 'a'.repeat(64),
      expectedRevision: 1,
      action: 'startEmptyWorkspace',
      idempotencyKey: 'not-a-uuid',
      payload: { conversationId, expectedWorkspaceRevision: null }
    })
    expect(invalid).toMatchObject({ success: false, code: 'VALIDATION_ERROR' })
    expect(invokeMock).not.toHaveBeenCalled()
  })
})
