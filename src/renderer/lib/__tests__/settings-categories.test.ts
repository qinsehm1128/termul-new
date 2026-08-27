import { describe, expect, it } from 'vitest'
import { isSettingsCategoryAvailable } from '../settings-categories'

describe('isSettingsCategoryAvailable', () => {
  it('hides the macOS privacy category off macOS', () => {
    expect(isSettingsCategoryAvailable('privacy', { isMac: false })).toBe(false)
    expect(isSettingsCategoryAvailable('privacy', { isMac: true })).toBe(true)
  })

  it('leaves every other category available on every platform', () => {
    for (const id of ['appearance', 'shell', 'updates', 'diagnostics', 'reset']) {
      expect(isSettingsCategoryAvailable(id, { isMac: false })).toBe(true)
      expect(isSettingsCategoryAvailable(id, { isMac: true })).toBe(true)
    }
  })

  it('does not treat an unknown id as platform-restricted', () => {
    // A category added later must show up by default; opting *out* is the
    // explicit act, so a missing entry cannot silently hide a whole section.
    expect(isSettingsCategoryAvailable('some-future-category', { isMac: false })).toBe(true)
  })
})
