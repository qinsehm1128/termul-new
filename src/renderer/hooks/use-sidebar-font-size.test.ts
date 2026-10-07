import { afterEach, describe, expect, it } from 'vitest'
import { applySidebarFontSize } from './use-sidebar-font-size'

describe('applySidebarFontSize', () => {
  afterEach(() => {
    document.documentElement.removeAttribute('style')
  })

  it("keeps today's sizes at the default step", () => {
    applySidebarFontSize(12)

    const style = document.documentElement.style
    expect(style.getPropertyValue('--sidebar-row-font-size')).toBe('12px')
    expect(style.getPropertyValue('--tree-row-font-size')).toBe('14px')
  })

  it('moves the project list and the file tree together', () => {
    applySidebarFontSize(17)

    const style = document.documentElement.style
    expect(style.getPropertyValue('--sidebar-row-font-size')).toBe('17px')
    expect(style.getPropertyValue('--tree-row-font-size')).toBe('19px')
  })
})
