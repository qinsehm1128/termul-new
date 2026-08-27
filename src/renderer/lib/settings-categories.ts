/**
 * Platform gating for settings categories.
 *
 * A category has three call sites — the sidebar list, the search index, and the
 * section itself — and they must agree. If search could reach an entry whose
 * section is not rendered, selecting the result scrolls to nothing; if the
 * sidebar listed it, clicking would do the same. One predicate for all three so
 * the three cannot drift apart.
 */

/** Categories that exist on exactly one platform, keyed by category id. */
const PLATFORM_ONLY_CATEGORIES: Record<string, 'mac'> = {
  // Reports macOS TCC grants; nothing on Windows or Linux corresponds to it.
  privacy: 'mac'
}

export interface SettingsHost {
  isMac: boolean
}

/** Whether `categoryId` should be listed, searchable and rendered on this host. */
export function isSettingsCategoryAvailable(categoryId: string, host: SettingsHost): boolean {
  const requires = PLATFORM_ONLY_CATEGORIES[categoryId]
  if (requires === undefined) return true
  return requires === 'mac' && host.isMac
}
