import { beforeEach, expect, it, vi } from 'vitest'
import { HTTP_IPC_NETWORK_ERROR_MESSAGE } from './http-ipc-result'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }))

import { createTauriConversationApi } from './tauri-conversation-api'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

beforeEach(() => {
  invokeMock.mockReset()
})

it('tauri conversation facade rejects malformed success envelope', async () => {
  const api = createTauriConversationApi()
  invokeMock.mockResolvedValue({ success: true, data: { extra: true } })

  const list = await api.listConversations()
  const host = await api.getHostStatus()
  const got = await api.getConversation(conversationId)
  const opened = await api.openConversation(conversationId)

  expect(list).toEqual({
    success: false,
    error: HTTP_IPC_NETWORK_ERROR_MESSAGE,
    code: 'NETWORK_ERROR'
  })
  expect(host).toEqual(list)
  expect(got).toEqual(list)
  expect(opened).toEqual(list)
  expect(list).not.toHaveProperty('data')
  expect(JSON.stringify({ list, host, got, opened })).not.toContain('extra')
})
