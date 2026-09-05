/**
 * Platform gating for settings categories.
 *
 * A category has three call sites — the sidebar list, the search index, and the
 * section itself — and they must agree. If search could reach an entry whose
 * section is not rendered, selecting the result scrolls to nothing; if the
 * sidebar listed it, clicking would do the same. One predicate for all three so
 * the three cannot drift apart.
 */

/** Categories that exist on exactly one kind of host, keyed by category id. */
const PLATFORM_ONLY_CATEGORIES: Record<string, 'mac' | 'desktop'> = {
  // Reports macOS TCC grants; nothing on Windows or Linux corresponds to it.
  privacy: 'mac',
  // Reads and copies roots on the machine the app is installed on. A browser
  // client has no pre-rename desktop install of its own to merge.
  'data-migration': 'desktop'
}

export interface SettingsHost {
  isMac: boolean
  /** Running inside the Tauri shell rather than a browser/remote client. */
  isDesktop: boolean
}

/** Whether `categoryId` should be listed, searchable and rendered on this host. */
export function isSettingsCategoryAvailable(categoryId: string, host: SettingsHost): boolean {
  const requires = PLATFORM_ONLY_CATEGORIES[categoryId]
  if (requires === undefined) return true
  if (requires === 'mac') return host.isMac
  return host.isDesktop
}
