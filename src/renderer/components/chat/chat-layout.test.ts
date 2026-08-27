import { describe, expect, it } from 'vitest'
import {
  CHAT_CONTENT_WIDTH,
  CHAT_STREAM_PAD_Y,
  CHAT_USER_MEASURE,
  chatTimelineRowClass,
  NARROW_PANE_PX,
  resolveComposerToolbarMode
} from './chat-layout'

describe('resolveComposerToolbarMode (Story 5.1)', () => {
  it('treats non-positive widths as wide (jsdom / pre-layout default)', () => {
    expect(resolveComposerToolbarMode(0)).toBe('wide')
    expect(resolveComposerToolbarMode(-1)).toBe('wide')
  })

  it('uses narrow below the pane threshold', () => {
    expect(resolveComposerToolbarMode(399)).toBe('narrow')
    expect(resolveComposerToolbarMode(375)).toBe('narrow')
    expect(resolveComposerToolbarMode(NARROW_PANE_PX - 1)).toBe('narrow')
  })

  it('uses wide at and above the pane threshold', () => {
    expect(resolveComposerToolbarMode(NARROW_PANE_PX)).toBe('wide')
    expect(resolveComposerToolbarMode(500)).toBe('wide')
    expect(resolveComposerToolbarMode(800)).toBe('wide')
  })
})

describe('chat stream layout tokens', () => {
  it('keeps the thread column on max-w-3xl so composer/notices stay aligned', () => {
    expect(CHAT_CONTENT_WIDTH).toContain('max-w-3xl')
    expect(CHAT_CONTENT_WIDTH).toContain('mx-auto')
    expect(CHAT_STREAM_PAD_Y).toBe('py-3')
    expect(CHAT_USER_MEASURE).toContain('36rem')
  })

  it('maps timeline kinds onto a user / assistant / tool hierarchy', () => {
    expect(chatTimelineRowClass('message', 'user')).toBe('chat-timeline-row chat-timeline-row-user')
    expect(chatTimelineRowClass('message', 'agent')).toBe(
      'chat-timeline-row chat-timeline-row-agent'
    )
    expect(chatTimelineRowClass('activity')).toBe('chat-timeline-row chat-timeline-row-activity')
    expect(chatTimelineRowClass('tool')).toBe('chat-timeline-row chat-timeline-row-tool')
    expect(chatTimelineRowClass('thought-group')).toBe(
      'chat-timeline-row chat-timeline-row-thought'
    )
  })
})
