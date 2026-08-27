import { describe, expect, it } from 'vitest'
import { isHiddenRunningTerminal } from './project'

describe('isHiddenRunningTerminal', () => {
  it('keeps a hidden Conversation PTY in the current session list', () => {
    expect(
      isHiddenRunningTerminal(
        {
          ptyId: 'pty-1',
          viewState: 'hidden',
          conversationId: 'c1',
          projectId: 'p1'
        },
        { conversationId: 'c1', projectId: 'p1' }
      )
    ).toBe(true)
  })

  it('lists a hidden project shell only when it is not Conversation-scoped', () => {
    expect(
      isHiddenRunningTerminal(
        { ptyId: 'pty-2', viewState: 'hidden', projectId: 'p1' },
        { conversationId: null, projectId: 'p1' }
      )
    ).toBe(true)
    expect(
      isHiddenRunningTerminal(
        {
          ptyId: 'pty-3',
          viewState: 'hidden',
          conversationId: 'other',
          projectId: 'p1'
        },
        { conversationId: 'c1', projectId: 'p1' }
      )
    ).toBe(false)
  })
})
