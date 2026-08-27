import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  clearChatRoute,
  isConversationAreaPath,
  navigateToConversation,
  navigateToPath,
  setRouterNavigate
} from './router-navigate'

describe('router-navigate', () => {
  afterEach(() => {
    setRouterNavigate(null)
    window.location.hash = ''
  })

  it('treats the conversation list and open conversation as the conversation area', () => {
    expect(isConversationAreaPath('/conversations')).toBe(true)
    expect(isConversationAreaPath('/c/018f7a1c-1b4d-7c8a-9f01-0123456789ab')).toBe(true)
    expect(isConversationAreaPath('/')).toBe(false)
    expect(isConversationAreaPath('/settings')).toBe(false)
  })

  it('leaves an open conversation on the conversation list instead of the project workspace', () => {
    const navigate = vi.fn()
    setRouterNavigate(navigate)
    window.location.hash = '#/c/018f7a1c-1b4d-7c8a-9f01-0123456789ab'
    clearChatRoute()
    expect(navigate).toHaveBeenCalledWith('/conversations')
  })

  it('does not navigate when already outside a conversation route', () => {
    const navigate = vi.fn()
    setRouterNavigate(navigate)
    window.location.hash = '#/'
    clearChatRoute()
    expect(navigate).not.toHaveBeenCalled()
  })

  it('navigates to a hash route when the path is different', () => {
    const navigate = vi.fn()
    setRouterNavigate(navigate)
    window.location.hash = '#/'
    navigateToPath('/terminals')
    expect(navigate).toHaveBeenCalledWith('/terminals')
  })

  it('opens a conversation without rewriting an identical hash', () => {
    const navigate = vi.fn()
    setRouterNavigate(navigate)
    const id = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
    window.location.hash = `#/c/${id}`
    navigateToConversation(id)
    expect(navigate).not.toHaveBeenCalled()
  })
})
