import { describe, expect, it } from 'vitest'
import { conversationRowStatus } from './conversation-row-status'

const conversationId = 'conv-1'
const otherId = 'conv-2'

describe('conversationRowStatus', () => {
  it('is idle when nothing is bound or pending', () => {
    expect(conversationRowStatus(conversationId, {}, [], {}, {})).toBe('idle')
  })

  it('is need when a pending permission belongs to a live bound session', () => {
    expect(
      conversationRowStatus(
        conversationId,
        { s1: { conversationId, activeTurn: false, status: 'active' } },
        [],
        { req: { sessionId: 's1' } },
        {}
      )
    ).toBe('need')
  })

  it('is need when a pending question belongs to a session bound via the index', () => {
    expect(
      conversationRowStatus(
        conversationId,
        {},
        [{ id: 's-index', conversationId }],
        {},
        { q1: { sessionId: 's-index' } }
      )
    ).toBe('need')
  })

  it('treats need as higher priority than a live working turn', () => {
    expect(
      conversationRowStatus(
        conversationId,
        { s1: { conversationId, activeTurn: true, status: 'active' } },
        [{ id: 's1', conversationId }],
        { req: { sessionId: 's1' } }
      )
    ).toBe('need')
  })

  it('is working when a live bound session has an active turn', () => {
    expect(
      conversationRowStatus(
        conversationId,
        { s1: { conversationId, activeTurn: true, status: 'active' } },
        [],
        {},
        {}
      )
    ).toBe('working')
  })

  it('is working when a live bound session is initializing', () => {
    expect(
      conversationRowStatus(
        conversationId,
        { s1: { conversationId, activeTurn: false, status: 'initializing' } },
        [{ id: 's1', conversationId }],
        {}
      )
    ).toBe('working')
  })

  it('uses the index to bind a live session that omitted conversationId', () => {
    expect(
      conversationRowStatus(
        conversationId,
        { s1: { activeTurn: true, status: 'active' } },
        [{ id: 's1', conversationId }],
        {}
      )
    ).toBe('working')
  })

  it('does not treat index-only initializing as working', () => {
    expect(
      conversationRowStatus(conversationId, {}, [{ id: 's-index', conversationId }], {}, {})
    ).toBe('idle')
  })

  it('ignores pending items and live turns bound to another conversation', () => {
    expect(
      conversationRowStatus(
        conversationId,
        {
          other: { conversationId: otherId, activeTurn: true, status: 'initializing' }
        },
        [{ id: 'other', conversationId: otherId }],
        { req: { sessionId: 'other' } },
        { q: { sessionId: 'other' } }
      )
    ).toBe('idle')
  })

  it('is idle for a live bound session that is not turning or initializing', () => {
    expect(
      conversationRowStatus(
        conversationId,
        { s1: { conversationId, activeTurn: false, status: 'active' } },
        [{ id: 's1', conversationId }],
        {},
        {}
      )
    ).toBe('idle')
  })
})
