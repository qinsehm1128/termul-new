import { useEffect, useSyncExternalStore } from 'react'
import {
  applyColorTheme,
  DEFAULT_COLOR_THEME_ID,
  getCustomColorThemesCacheVersion,
  getEffectiveThemeId,
  normalizeThemeFamilyId,
  subscribeCustomColorThemes
} from '@/lib/themes'
import { useAppearanceMode, useAppSettingsLoaded, useColorTheme } from '@/stores/app-settings-store'
import { useThemePickerOpen } from '@/stores/theme-picker-store'

/** Keep the applied (persisted) color theme in sync — skips while the picker is previewing. */
export function useAppliedColorThemeSync(): void {
  const isLoaded = useAppSettingsLoaded()
  const colorTheme = useColorTheme()
  const appearanceMode = useAppearanceMode()
  const isPickerOpen = useThemePickerOpen()
  const customThemesVersion = useSyncExternalStore(
    subscribeCustomColorThemes,
    getCustomColorThemesCacheVersion,
    getCustomColorThemesCacheVersion
  )

  useEffect(() => {
    if (!isLoaded || isPickerOpen) return
    // Re-resolve once the imported themes land: the persisted id is read before
    // the registry is populated, so a custom theme would otherwise sit on
    // `getColorThemeDefinition`'s silent default fallback until the next change.
    void customThemesVersion
    const familyId = normalizeThemeFamilyId(colorTheme) || DEFAULT_COLOR_THEME_ID
    const themeId = getEffectiveThemeId(familyId, appearanceMode)
    applyColorTheme(themeId)
  }, [isLoaded, colorTheme, appearanceMode, isPickerOpen, customThemesVersion])
}

export function useEffectiveColorThemeId(): string {
  const colorTheme = useColorTheme()
  const appearanceMode = useAppearanceMode()
  const familyId = normalizeThemeFamilyId(colorTheme) || DEFAULT_COLOR_THEME_ID
  return getEffectiveThemeId(familyId, appearanceMode)
}
