import { describe, expect, it } from 'vitest'
import { isSettingsCategoryAvailable, type SettingsHost } from '../settings-categories'

const MAC_DESKTOP: SettingsHost = { isMac: true, isDesktop: true }
const LINUX_DESKTOP: SettingsHost = { isMac: false, isDesktop: true }
const MAC_BROWSER: SettingsHost = { isMac: true, isDesktop: false }
const LINUX_BROWSER: SettingsHost = { isMac: false, isDesktop: false }

describe('isSettingsCategoryAvailable', () => {
  it('hides the macOS privacy category off macOS', () => {
    expect(isSettingsCategoryAvailable('privacy', LINUX_DESKTOP)).toBe(false)
    expect(isSettingsCategoryAvailable('privacy', MAC_DESKTOP)).toBe(true)
  })

  /**
   * The panel reads roots on the machine the app is installed on. A browser
   * client has no pre-rename desktop install of its own, and every command
   * behind the panel refuses outside Tauri — so listing it there would offer a
   * button that cannot work.
   */
  it('hides the data-migration category outside the desktop shell', () => {
    expect(isSettingsCategoryAvailable('data-migration', MAC_BROWSER)).toBe(false)
    expect(isSettingsCategoryAvailable('data-migration', LINUX_BROWSER)).toBe(false)
    expect(isSettingsCategoryAvailable('data-migration', MAC_DESKTOP)).toBe(true)
    expect(isSettingsCategoryAvailable('data-migration', LINUX_DESKTOP)).toBe(true)
  })

  it('leaves every other category available on every host', () => {
    for (const id of ['appearance', 'shell', 'updates', 'diagnostics', 'reset']) {
      for (const host of [MAC_DESKTOP, LINUX_DESKTOP, MAC_BROWSER, LINUX_BROWSER]) {
        expect(isSettingsCategoryAvailable(id, host)).toBe(true)
      }
    }
  })

  it('does not treat an unknown id as platform-restricted', () => {
    // A category added later must show up by default; opting *out* is the
    // explicit act, so a missing entry cannot silently hide a whole section.
    expect(isSettingsCategoryAvailable('some-future-category', LINUX_BROWSER)).toBe(true)
  })
})
