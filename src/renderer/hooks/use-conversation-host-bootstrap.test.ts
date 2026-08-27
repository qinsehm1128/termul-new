import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { conversationApi } from '@/lib/conversation-api'
import { useConversationStore } from '@/stores/conversation-store'
import {
  useConversationHostBootstrap,
  useConversationHostBootstrapStore
} from './use-conversation-host-bootstrap'

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: {
    getHostStatus: vi.fn(),
    listConversations: vi.fn(),
    subscribeHostStatus: vi.fn()
  }
}))

const status = {
  hostKind: 'desktop' as const,
  state: 'ready' as const,
  code: 'CONVERSATION_HOST_READY',
  migrationPhase: 'finalized' as const,
  readerPrecedence: 'conversationV2Only' as const,
  recoveryItemCount: 0,
  recoveryItems: []
}

beforeEach(() => {
  vi.clearAllMocks()
  useConversationHostBootstrapStore.getState().reset()
  useConversationStore.getState().reset()
  vi.mocked(conversationApi.getHostStatus).mockResolvedValue({ success: true, data: status })
  vi.mocked(conversationApi.listConversations).mockResolvedValue({ success: true, data: [] })
  vi.mocked(conversationApi.subscribeHostStatus).mockReturnValue(() => undefined)
})

describe('useConversationHostBootstrap', () => {
  it('loads host status, conversations, and the recovery queue', async () => {
    renderHook(() => useConversationHostBootstrap())
    await waitFor(() => expect(useConversationHostBootstrapStore.getState().loading).toBe(false))
    expect(useConversationHostBootstrapStore.getState().status).toEqual(status)
    expect(conversationApi.listConversations).toHaveBeenCalledTimes(1)
    expect(useConversationStore.getState().conversationIds).toEqual([])
  })

  it('renders a stable error code instead of throwing a platform stub', async () => {
    vi.mocked(conversationApi.getHostStatus).mockResolvedValue({
      success: false,
      code: 'FORBIDDEN',
      error: 'denied'
    })
    renderHook(() => useConversationHostBootstrap())
    await waitFor(() => expect(useConversationHostBootstrapStore.getState().loading).toBe(false))
    expect(useConversationHostBootstrapStore.getState().status?.state).toBe('error')
    expect(useConversationHostBootstrapStore.getState().status?.code).toBe('FORBIDDEN')
  })

  it('refreshes both surfaces after a reconnect notification', async () => {
    let reconnect: (() => void) | undefined
    vi.mocked(conversationApi.subscribeHostStatus).mockImplementation((listener) => {
      reconnect = listener
      return () => undefined
    })
    renderHook(() => useConversationHostBootstrap())
    await waitFor(() => expect(conversationApi.getHostStatus).toHaveBeenCalledTimes(1))
    act(() => reconnect?.())
    await waitFor(() => expect(conversationApi.getHostStatus).toHaveBeenCalledTimes(2))
  })
})
