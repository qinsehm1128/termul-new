import { describe, expect, it } from 'vitest'
import { closingKeepsProcessAlive, terminalCloseIntent } from './terminal-close-intent'

const CONVERSATION_ID = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

describe('closingKeepsProcessAlive', () => {
  it('is false for a project shell', () => {
    // Nothing else owns it, so a tab close that left it running would leak it.
    expect(closingKeepsProcessAlive({ conversationId: undefined })).toBe(false)
  })

  it('is true for a terminal an agent opened', () => {
    expect(closingKeepsProcessAlive({ conversationId: CONVERSATION_ID })).toBe(true)
  })
})

describe('terminalCloseIntent', () => {
  it('asks before killing by default', () => {
    // Two separate settings on purpose: the view-close opt-out promises the
    // process keeps running, so it must not silence the prompt for a kill.
    expect(terminalCloseIntent({ conversationId: undefined }, false, true)).toBe(
      'terminate-confirm'
    )
    expect(terminalCloseIntent({ conversationId: undefined }, true, true)).toBe('terminate-confirm')
  })

  it('kills without asking once the user turns that confirmation off', () => {
    expect(terminalCloseIntent({ conversationId: undefined }, true, false)).toBe('terminate')
  })

  it('honours the view-close setting for a close that keeps the process', () => {
    expect(terminalCloseIntent({ conversationId: CONVERSATION_ID }, true, true)).toBe(
      'close-view-confirm'
    )
    expect(terminalCloseIntent({ conversationId: CONVERSATION_ID }, false, true)).toBe('close-view')
  })

  it('never kills a view close, whatever the terminate setting says', () => {
    expect(terminalCloseIntent({ conversationId: CONVERSATION_ID }, true, false)).toBe(
      'close-view-confirm'
    )
  })

  it('asks before killing a terminal whose record has already gone', () => {
    expect(terminalCloseIntent(undefined, false, true)).toBe('terminate-confirm')
  })
})
