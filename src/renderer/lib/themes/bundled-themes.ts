import { brandCanonical, LEGACY } from '@shared/brand'
import { BRAND_LIGHT_THEME, BUNDLED_LIGHT_COLOR_THEMES } from './bundled-light-themes'
import { getCustomColorTheme, getCustomColorThemes } from './custom-theme-registry'
import type { ColorThemeDefinition } from './types'

export interface ColorThemeFamily {
  familyId: string
  name: string
  darkThemeId: string
  lightThemeId: string
}

/**
 * The bundled brand theme (dark), and the default.
 *
 * Its ids come from the brand seam, not from authored literals — the table
 * below is keyed *by* the theme id, so an authored spelling would be a second
 * home for a contract `brand.ts` already owns. {@link resolvedColorThemes}
 * anchors the compatibility alias on this binding rather than on a key lookup.
 */
const BRAND_DARK_THEME: ColorThemeDefinition = {
  // syntax: VS Code Dark+ (Se default editor)
  id: brandCanonical().themeId,
  name: 'Se',
  appearance: 'dark',
  familyId: brandCanonical().themeId,
  dark: {
    palette: {
      neutral: '#131410',
      ink: '#e6e5e0',
      primary: '#8a9d72',
      accent: '#8a9d72',
      success: '#739d6c',
      warning: '#c39f69',
      error: '#c26b6b',
      info: '#7898ab'
    },
    overrides: {
      'syntax-comment': '#6a9955',
      'syntax-keyword': '#c586c0',
      'syntax-string': '#ce9178',
      'syntax-type': '#4ec9b0',
      'syntax-constant': '#b5cea8',
      'syntax-variable': '#9cdcfe',
      'syntax-property': '#9cdcfe',
      'syntax-function': '#dcdcaa'
    }
  }
}

/** Built-in dark appearance themes (OpenCode palette-compatible). */
const BUNDLED_DARK_COLOR_THEMES: Record<string, ColorThemeDefinition> = {
  [BRAND_DARK_THEME.id]: BRAND_DARK_THEME,
  // syntax: opencode cursor + vscode fallback
  cursor: {
    id: 'cursor',
    name: 'Cursor',
    appearance: 'dark',
    familyId: 'cursor',
    dark: {
      palette: {
        neutral: '#181818',
        ink: '#e4e4e4',
        primary: '#88c0d0',
        accent: '#f38ba8',
        success: '#3fa266',
        warning: '#f1b467',
        error: '#e34671',
        info: '#81a1c1'
      },
      overrides: {
        'syntax-comment': '#e4e4e45e',
        'syntax-keyword': '#82d2ce',
        'syntax-string': '#e394dc',
        'syntax-type': '#efb080',
        'syntax-constant': '#f8c762',
        'syntax-property': '#81a1c1',
        'syntax-function': '#efb080'
      }
    }
  },
  // syntax: opencode catppuccin mocha + vscode fallback
  catppuccin: {
    id: 'catppuccin',
    name: 'Catppuccin',
    appearance: 'dark',
    familyId: 'catppuccin',
    dark: {
      palette: {
        neutral: '#1e1e2e',
        ink: '#cdd6f4',
        primary: '#b4befe',
        accent: '#f38ba8',
        success: '#a6e3a1',
        warning: '#fab387',
        error: '#f38ba8',
        info: '#89dceb'
      },
      overrides: {
        'syntax-comment': '#6c7086',
        'syntax-keyword': '#cba6f7',
        'syntax-string': '#a6e3a1',
        'syntax-primitive': '#89b4fa',
        'syntax-constant': '#fab387',
        'syntax-type': '#f9e2af',
        'syntax-function': '#89b4fa'
      }
    }
  },
  // syntax: opencode dracula
  dracula: {
    id: 'dracula',
    name: 'Dracula',
    appearance: 'dark',
    familyId: 'dracula',
    dark: {
      palette: {
        neutral: '#1d1e28',
        ink: '#f8f8f2',
        primary: '#bd93f9',
        accent: '#ff79c6',
        success: '#50fa7b',
        warning: '#ffb86c',
        error: '#ff5555',
        info: '#8be9fd'
      },
      overrides: {
        'syntax-comment': '#6272a4',
        'syntax-keyword': '#ff79c6',
        'syntax-string': '#f1fa8c',
        'syntax-primitive': '#50fa7b',
        'syntax-constant': '#bd93f9',
        'syntax-property': '#8be9fd',
        'syntax-function': '#50fa7b'
      }
    }
  },
  // syntax: opencode nord + vscode fallback
  nord: {
    id: 'nord',
    name: 'Nord',
    appearance: 'dark',
    familyId: 'nord',
    dark: {
      palette: {
        neutral: '#2e3440',
        ink: '#eceff4',
        primary: '#88c0d0',
        accent: '#bf616a',
        success: '#a3be8c',
        warning: '#ebcb8b',
        error: '#bf616a',
        info: '#81a1c1'
      },
      overrides: {
        'syntax-comment': '#616e88',
        'syntax-keyword': '#81a1c1',
        'syntax-string': '#a3be8c',
        'syntax-type': '#8fbcbb',
        'syntax-constant': '#b48ead',
        'syntax-function': '#88c0d0'
      }
    }
  },
  // syntax: opencode gruvbox
  gruvbox: {
    id: 'gruvbox',
    name: 'Gruvbox',
    appearance: 'dark',
    familyId: 'gruvbox',
    dark: {
      palette: {
        neutral: '#282828',
        ink: '#ebdbb2',
        primary: '#83a598',
        accent: '#d3869b',
        success: '#b8bb26',
        warning: '#fabd2f',
        error: '#fb4934',
        info: '#8ec07c'
      },
      overrides: {
        'syntax-comment': '#928374',
        'syntax-keyword': '#fb4934',
        'syntax-string': '#b8bb26',
        'syntax-type': '#fabd2f',
        'syntax-constant': '#d3869b',
        'syntax-function': '#83a598'
      }
    }
  },
  // syntax: opencode tokyonight
  tokyonight: {
    id: 'tokyonight',
    name: 'Tokyo Night',
    appearance: 'dark',
    familyId: 'tokyonight',
    dark: {
      palette: {
        neutral: '#1a1b26',
        ink: '#c0caf5',
        primary: '#7aa2f7',
        accent: '#bb9af7',
        success: '#9ece6a',
        warning: '#e0af68',
        error: '#f7768e',
        info: '#7dcfff'
      },
      overrides: {
        'syntax-comment': '#565f89',
        'syntax-keyword': '#bb9af7',
        'syntax-string': '#9ece6a',
        'syntax-type': '#2ac3de',
        'syntax-constant': '#ff9e64',
        'syntax-property': '#7dcfff',
        'syntax-function': '#7aa2f7'
      }
    }
  },
  // syntax: opencode ayu dark
  ayu: {
    id: 'ayu',
    name: 'Ayu',
    appearance: 'dark',
    familyId: 'ayu',
    dark: {
      palette: {
        neutral: '#0b0e14',
        ink: '#bfbdb6',
        primary: '#39bae6',
        accent: '#ff8f40',
        success: '#7fd962',
        warning: '#ffb454',
        error: '#f26d78',
        info: '#59c2ff'
      },
      overrides: {
        'syntax-comment': '#5a6673',
        'syntax-keyword': '#ff8f40',
        'syntax-string': '#aad94c',
        'syntax-type': '#59c2ff',
        'syntax-constant': '#d2a6ff',
        'syntax-property': '#39bae6',
        'syntax-function': '#ffb454'
      }
    }
  },
  // syntax: opencode one-dark
  'one-dark': {
    id: 'one-dark',
    name: 'One Dark',
    appearance: 'dark',
    familyId: 'one-dark',
    dark: {
      palette: {
        neutral: '#282c34',
        ink: '#abb2bf',
        primary: '#61afef',
        accent: '#c678dd',
        success: '#98c379',
        warning: '#e5c07b',
        error: '#e06c75',
        info: '#56b6c2'
      },
      overrides: {
        'syntax-comment': '#5c6370',
        'syntax-keyword': '#c678dd',
        'syntax-string': '#98c379',
        'syntax-type': '#e5c07b',
        'syntax-constant': '#d19a66',
        'syntax-variable': '#e06c75',
        'syntax-property': '#56b6c2',
        'syntax-function': '#61afef'
      }
    }
  },
  // syntax: opencode github dark
  github: {
    id: 'github',
    name: 'GitHub',
    appearance: 'dark',
    familyId: 'github',
    dark: {
      palette: {
        neutral: '#0d1117',
        ink: '#c9d1d9',
        primary: '#58a6ff',
        accent: '#f78166',
        success: '#3fb950',
        warning: '#d29922',
        error: '#f85149',
        info: '#79c0ff'
      },
      overrides: {
        'syntax-comment': '#8b949e',
        'syntax-keyword': '#ff7b72',
        'syntax-string': '#39c5cf',
        'syntax-type': '#d29922',
        'syntax-constant': '#79c0ff',
        'syntax-variable': '#d29922',
        'syntax-property': '#39c5cf',
        'syntax-function': '#bc8cff'
      }
    }
  }
}

export const BUNDLED_COLOR_THEMES: Record<string, ColorThemeDefinition> = {
  ...BUNDLED_DARK_COLOR_THEMES,
  ...BUNDLED_LIGHT_COLOR_THEMES
}

/**
 * The bundled dark/light pairs the picker groups by.
 *
 * A function rather than a module `const`: a table evaluated at import time is
 * frozen before {@link getCustomColorThemes} can hold anything and before the
 * brand seam can move, which is the same failure `resolvedColorThemes` exists
 * to prevent one contract over.
 */
export function colorThemeFamilies(): ColorThemeFamily[] {
  return Object.values(BUNDLED_DARK_COLOR_THEMES).map((theme) => ({
    familyId: theme.familyId,
    name: theme.name.replace(/ Light$/, ''),
    darkThemeId: theme.id,
    lightThemeId: `${theme.familyId}-light`
  }))
}

export interface ThemePickerRow {
  themeId: string
  familyId: string
  label: string
  variant: 'dark' | 'light'
  /**
   * Which table the row came from. Custom rows are their own section: they
   * have no light twin, so they cannot be folded into a bundled family.
   */
  source: 'bundled' | 'custom'
}

/**
 * Every row the picker offers — bundled pairs first, then the user's imported
 * themes in import order.
 *
 * Takes the custom list as an argument so React can key a memo on the value it
 * already read from {@link subscribeCustomColorThemes}; the default keeps
 * non-React callers to a bare call.
 */
export function themePickerRows(
  customThemes: readonly ColorThemeDefinition[] = getCustomColorThemes()
): ThemePickerRow[] {
  const bundled = colorThemeFamilies().flatMap((family): ThemePickerRow[] => [
    {
      themeId: family.darkThemeId,
      familyId: family.familyId,
      label: family.name,
      variant: 'dark',
      source: 'bundled'
    },
    {
      themeId: family.lightThemeId,
      familyId: family.familyId,
      label: `${family.name} Light`,
      variant: 'light',
      source: 'bundled'
    }
  ])

  const custom = customThemes.map(
    (theme): ThemePickerRow => ({
      themeId: theme.id,
      familyId: theme.familyId,
      label: theme.name,
      variant: theme.appearance,
      source: 'custom'
    })
  )

  return [...bundled, ...custom]
}

export const DEFAULT_COLOR_THEME_ID = BRAND_DARK_THEME.id

export const COLOR_THEME_LIST = Object.values(BUNDLED_COLOR_THEMES)

/** Memo for {@link resolvedColorThemes}, keyed by the two brand ids it bakes in. */
let cachedBrandThemeKey: string | null = null
let cachedResolvedThemes: Record<string, ColorThemeDefinition> | null = null

/**
 * The bundled themes keyed by every id that must resolve today: the brand
 * theme's canonical id and its light twin, plus the ids a pre-rename install
 * persisted. Both spellings answer with the *canonical* identity, so a user
 * who picked the theme before the rename keeps it rather than being dropped
 * onto the default by `getColorThemeDefinition`'s silent fallback.
 *
 * Built here rather than captured at module scope: `brandCanonical()` is an
 * overridable seam, and a table frozen at import time would still answer with
 * the pre-rename identity after the seam moved. Memoized on the two ids so a
 * lookup stays a hash probe.
 *
 * The two definitions are taken from the `BRAND_*_THEME` bindings, not from a
 * key lookup into the authored table: the authored keys are themselves derived
 * from the seam, so a lookup would miss the moment the seam moved.
 */
function resolvedColorThemes(): Record<string, ColorThemeDefinition> {
  const { themeId, themeFamilyLight } = brandCanonical()

  // Joined on a control char that cannot occur in a theme id, so no pair of
  // ids can collide into the same memo key.
  const brandKey = `${themeId}\u0000${themeFamilyLight}`
  if (cachedResolvedThemes !== null && cachedBrandThemeKey === brandKey) {
    return cachedResolvedThemes
  }

  const dark: ColorThemeDefinition = {
    ...BRAND_DARK_THEME,
    id: themeId,
    familyId: themeId
  }
  const light: ColorThemeDefinition = {
    ...BRAND_LIGHT_THEME,
    id: themeFamilyLight,
    familyId: themeId
  }
  const resolved: Record<string, ColorThemeDefinition> = {
    ...BUNDLED_COLOR_THEMES,
    [LEGACY.themeId]: dark,
    [themeId]: dark,
    [LEGACY.themeFamilyLight]: light,
    [themeFamilyLight]: light
  }

  cachedBrandThemeKey = brandKey
  cachedResolvedThemes = resolved
  return resolved
}

/**
 * Whether `themeId` names a bundled theme — under its canonical id or under
 * the id a pre-rename install persisted.
 */
export function hasBundledColorTheme(themeId: string): boolean {
  return Object.prototype.hasOwnProperty.call(resolvedColorThemes(), themeId)
}

/** Whether `themeId` resolves to a real theme — bundled or user-imported. */
export function hasColorThemeDefinition(themeId: string): boolean {
  return hasBundledColorTheme(themeId) || getCustomColorTheme(themeId) !== undefined
}

/**
 * The single resolution funnel: every theme consumer in the renderer reaches a
 * definition through here.
 *
 * Bundled is probed first and the user registry second, so an imported theme
 * can never shadow a shipped one no matter how it got into the registry. The
 * fallback to the default theme stays silent, as before — `hasColorThemeDefinition`
 * is the predicate that distinguishes a hit from that fallback.
 */
export function getColorThemeDefinition(themeId: string): ColorThemeDefinition {
  const themes = resolvedColorThemes()
  if (Object.prototype.hasOwnProperty.call(themes, themeId)) {
    return themes[themeId]
  }
  return getCustomColorTheme(themeId) ?? themes[DEFAULT_COLOR_THEME_ID]
}
