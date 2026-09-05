import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import { describe, expect, it } from 'vitest'
import {
  displayConversationTitle,
  mergeConversationTitle,
  sessionTitleForConversation
} from './conversation-title'

const conversationId = '81a7dc2f-856b-4150-8f01-0123456789ab'

function record(overrides: Partial<ConversationRecordV2> = {}): ConversationRecordV2 {
  return {
    schemaVersion: 2,
    conversationId,
    createdAtUtc: '2026-08-19T00:00:00.000Z',
    creationPartition: { year: 2026, month: 8, day: 19, path: '2026/08/19' },
    workspaceCwd: `/conversations/2026/08/19/${conversationId}`,
    executionTarget: { kind: 'workspace' },
    projectAttachment: null,
    lifecycleState: 'ready',
    lastSeq: 1,
    createdBy: 'se-manager',
    ...overrides
  }
}

describe('displayConversationTitle', () => {
  it('uses a stored title before workspace or id fallbacks', () => {
    expect(
      displayConversationTitle(record({ title: 'bi查询demo' }), { untitled: '未命名对话' })
    ).toBe('bi查询demo')
  })

  it('uses the ACP session title when the record is untitled', () => {
    expect(
      displayConversationTitle(record({ title: null }), {
        sessionTitle: 'Fix iframe layout',
        untitled: '未命名对话'
      })
    ).toBe('Fix iframe layout')
  })

  it('does not show a UUID workspace folder as the list title', () => {
    expect(displayConversationTitle(record({ title: null }), { untitled: '未命名对话' })).toBe(
      '未命名对话'
    )
  })
})

describe('sessionTitleForConversation', () => {
  it('prefers a live session title, then the history index', () => {
    expect(
      sessionTitleForConversation(conversationId, { s1: { conversationId, title: 'Live title' } }, [
        { conversationId, title: 'Index title' }
      ])
    ).toBe('Live title')
    expect(
      sessionTitleForConversation(conversationId, {}, [{ conversationId, title: 'Index title' }])
    ).toBe('Index title')
  })
})

describe('mergeConversationTitle', () => {
  it('keeps the listed title when open returns an untitled record', () => {
    const listed = record({ title: 'bi查询demo', titleSource: 'derived_first_message' })
    const opened = record({ title: null, titleSource: null, lastSeq: 2 })
    expect(mergeConversationTitle(listed, opened)).toEqual({
      ...opened,
      title: 'bi查询demo',
      titleSource: 'derived_first_message'
    })
  })
})
