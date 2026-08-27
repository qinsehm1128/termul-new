import { describe, expect, it, vi } from 'vitest'
import { resolveConversationSessionId } from './conversation-binding'

vi.mock('@/lib/conversation-api', () => ({
  conversationApi: {
    getCurrentBinding: vi.fn()
  }
}))

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

describe('resolveConversationSessionId', () => {
  it('prefers a live session bound to the Conversation', () => {
    expect(
      resolveConversationSessionId(
        {
          sessions: {
            'opaque/live': { id: 'opaque/live', conversationId }
          },
          sessionIndex: [{ id: 'opaque/index', conversationId }]
        },
        conversationId
      )
    ).toBe('opaque/live')
  })

  it('falls back to the history index when no live session is bound', () => {
    expect(
      resolveConversationSessionId(
        {
          sessions: {},
          sessionIndex: [{ id: 'opaque/index', conversationId }]
        },
        conversationId
      )
    ).toBe('opaque/index')
  })

  it('returns null when the Conversation has no agent binding', () => {
    expect(
      resolveConversationSessionId(
        {
          sessions: {
            other: { id: 'other', conversationId: '028f7a1c-1b4d-7c8a-9f01-0123456789ab' }
          },
          sessionIndex: [{ id: 'opaque/index' }]
        },
        conversationId
      )
    ).toBeNull()
  })

  it('prefers a live session over a closed history session', () => {
    expect(
      resolveConversationSessionId(
        {
          sessions: {
            closed: { id: 'closed', conversationId, status: 'closed' },
            live: { id: 'live', conversationId, status: 'active' }
          },
          sessionIndex: [{ id: 'closed', conversationId }]
        },
        conversationId
      )
    ).toBe('live')
  })

  it('joins a history row whose host identity is storageKey', () => {
    expect(
      resolveConversationSessionId(
        {
          sessions: {},
          sessionIndex: [{ id: 'opaque/index', storageKey: conversationId }]
        },
        conversationId
      )
    ).toBe('opaque/index')
  })
})
