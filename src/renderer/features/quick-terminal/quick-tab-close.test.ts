import { describe, expect, it, vi } from 'vitest'
import { claimQuickTabClose, closeQuickTab, isQuickTerminalPath } from './quick-tab-close'

describe('quick tab close ownership', () => {
  it('only recognizes the quick list and its descendants', () => {
    expect(isQuickTerminalPath('/quick-terminals')).toBe(true)
    expect(isQuickTerminalPath('/quick-terminals/entry')).toBe(true)
    expect(isQuickTerminalPath('/quick-terminals-other')).toBe(false)
    expect(isQuickTerminalPath('/')).toBe(false)
  })

  it('releases only its own claim and never falls through to a previous owner', () => {
    const previous = vi.fn()
    const current = vi.fn()
    const releasePrevious = claimQuickTabClose(previous)
    const releaseCurrent = claimQuickTabClose(current)
    releasePrevious()
    closeQuickTab()
    expect(current).toHaveBeenCalledOnce()
    expect(previous).not.toHaveBeenCalled()
    releaseCurrent()
    closeQuickTab()
    expect(current).toHaveBeenCalledOnce()
    expect(previous).not.toHaveBeenCalled()
  })

  it('keeps a newer registration when both claims reuse the same callback', () => {
    const handler = vi.fn()
    const releaseOld = claimQuickTabClose(handler)
    const releaseNew = claimQuickTabClose(handler)
    releaseOld()
    closeQuickTab()
    expect(handler).toHaveBeenCalledOnce()
    releaseNew()
    closeQuickTab()
    expect(handler).toHaveBeenCalledOnce()
  })
})
