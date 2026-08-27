import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))

import type { ConversationHistoryPageV1 } from '@shared/types/web-protocol.types'
import { invoke } from '@tauri-apps/api/core'
import { AcpHistoryApiError, acpHistoryApi } from './acp-history-api'
import type { SessionPayload } from './acp-history-persistence'

const page: ConversationHistoryPageV1 = {
  schemaVersion: 1,
  records: [
    {
      schemaVersion: 1,
      sessionId: 's-1',
      seq: 18,
      type: 'message_chunk',
      recordedAt: 18,
      payload: { role: 'agent', content: { type: 'text', text: 'ok' } }
    }
  ],
  nextCursor: 18,
  complete: false,
  targetLastSeq: 42
}

const stored: SessionPayload = {
  metadata: {
    id: 's-1',
    agentId: 'a-1',
    title: 'Chat',
    cwd: '/p',
    projectId: 'p-1',
    createdAt: 1,
    lastActivityAt: 2,
    messageCount: 0,
    status: 'closed'
  },
  messages: []
}

beforeEach(() => vi.clearAllMocks())

describe('acpHistoryApi command contract', () => {
  it.each([
    ['list', 'acp_history_list', undefined, { sessions: [], legacyImportComplete: false }],
    ['get', 'acp_history_get', { sessionId: 's-1' }, stored],
    ['save', 'acp_history_save', { sessionId: 's-1', payload: stored }, undefined],
    ['delete', 'acp_history_delete', { sessionId: 's-1' }, undefined],
    ['flush', 'acp_history_flush', undefined, undefined],
    ['markLegacyImportComplete', 'acp_history_mark_legacy_import_complete', undefined, undefined]
  ] as const)('invokes %s with the exact command and args', async (method, command, args, data) => {
    vi.mocked(invoke).mockResolvedValueOnce({ success: true, data })
    if (method === 'get') await acpHistoryApi.get('s-1')
    else if (method === 'save') await acpHistoryApi.save('s-1', stored)
    else if (method === 'delete') await acpHistoryApi.delete('s-1')
    else await acpHistoryApi[method]()
    expect(invoke).toHaveBeenCalledWith(command, args)
  })

  it('getPage omits the target on page one and returns the exact page identity', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ success: true, data: page })

    const result = await acpHistoryApi.getPage('s-1', 0, 250)

    expect(invoke).toHaveBeenCalledTimes(1)
    expect(invoke).toHaveBeenCalledWith('acp_history_get_page', {
      sessionId: 's-1',
      afterSeq: 0,
      limit: 250
    })
    expect(result).toBe(page)
  })

  it('getPage forwards the exact pinned targetLastSeq on continuation pages', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ success: true, data: page })

    await expect(acpHistoryApi.getPage('s-1', 17, 250, 42)).resolves.toBe(page)
    expect(invoke).toHaveBeenCalledWith('acp_history_get_page', {
      sessionId: 's-1',
      afterSeq: 17,
      limit: 250,
      targetLastSeq: 42
    })
  })

  it('getPage preserves structured CONVERSATION_HISTORY_PAGING_REQUIRED failures', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      success: false,
      error: 'use bounded history pages',
      code: 'CONVERSATION_HISTORY_PAGING_REQUIRED'
    })

    await expect(acpHistoryApi.getPage('s-1', 17, 250)).rejects.toEqual(
      expect.objectContaining({
        name: 'AcpHistoryApiError',
        code: 'CONVERSATION_HISTORY_PAGING_REQUIRED',
        message: 'use bounded history pages'
      })
    )
  })

  it('getPage propagates a rejected invoke error unchanged', async () => {
    const rejection = new Error('invoke unavailable')
    vi.mocked(invoke).mockRejectedValueOnce(rejection)

    await expect(acpHistoryApi.getPage('s-1', 17, 250)).rejects.toBe(rejection)
  })

  it('getPage rejects invalid bounds and targets before invoking', async () => {
    await expect(acpHistoryApi.getPage('s-1', 0, 0)).rejects.toMatchObject({
      code: 'VALIDATION_ERROR'
    })
    await expect(acpHistoryApi.getPage('s-1', 17, 250, 16)).rejects.toMatchObject({
      code: 'VALIDATION_ERROR'
    })
    await expect(acpHistoryApi.getPage('s-1', 17, 250, Number.NaN)).rejects.toMatchObject({
      code: 'VALIDATION_ERROR'
    })
    expect(invoke).not.toHaveBeenCalled()
  })

  it('surfaces structured command failures', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      success: false,
      error: 'disk full',
      code: 'ACP_HISTORY_SAVE_FAILED'
    })
    await expect(acpHistoryApi.save('s-1', stored)).rejects.toEqual(
      new AcpHistoryApiError('ACP_HISTORY_SAVE_FAILED', 'disk full')
    )
  })

  it('surfaces invoke transport failures', async () => {
    const rejection = new Error('invoke unavailable')
    vi.mocked(invoke).mockRejectedValueOnce(rejection)
    await expect(acpHistoryApi.list()).rejects.toBe(rejection)
  })
})
