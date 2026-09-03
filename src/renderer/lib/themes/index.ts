export {
  applyColorTheme,
  getActiveTerminalTheme,
  getActiveTerminalThemeId,
  getLastAppliedColorThemeId,
  isKnownColorThemeId
} from './apply-color-theme'
export { applyThemeToTerminal } from './apply-theme-to-terminal'
export {
  BUNDLED_COLOR_THEMES,
  COLOR_THEME_LIST,
  type ColorThemeFamily,
  colorThemeFamilies,
  DEFAULT_COLOR_THEME_ID,
  getColorThemeDefinition,
  hasColorThemeDefinition,
  type ThemePickerRow,
  themePickerRows
} from './bundled-themes'
export {
  getCustomColorTheme,
  getCustomColorThemes,
  getCustomColorThemesCacheVersion,
  subscribeCustomColorThemes
} from './custom-theme-registry'
export {
  type CustomThemeError,
  type CustomThemeErrorCode,
  type CustomThemeResult,
  deleteCustomColorTheme,
  exportCustomColorTheme,
  importCustomColorTheme,
  loadCustomColorThemes,
  mergeCustomColorThemes,
  parseCustomColorTheme,
  validateCustomColorTheme
} from './custom-themes'
export { deriveSurfaces } from './derive-surfaces'
export { resolveSyntaxColors } from './resolve-syntax'
export {
  type AppearanceMode,
  getEffectiveThemeId,
  getLightThemeId,
  getPickerApplySettings,
  getSystemAppearance,
  normalizeThemeFamilyId,
  type ThemeAppearance
} from './theme-appearance'
export type {
  ColorThemeChangedDetail,
  ColorThemeDefinition,
  ResolvedSyntaxColors,
  ThemePalette
} from './types'
export { COLOR_THEME_CHANGED_EVENT } from './types'
