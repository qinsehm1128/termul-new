/**
 * In-memory registry of the user's imported color themes.
 *
 * Split out of `custom-themes.ts` so the resolver can consult it without an
 * import cycle: `bundled-themes.ts` needs the registry, and the import /
 * validation layer needs `bundled-themes.ts` for the id set it dedups against.
 *
 * Mirrors the custom-agent cache (`lib/agents/custom-agents.ts:22-44`): a
 * module-level map plus a version counter, consumed from React through
 * `useSyncExternalStore` so the picker re-renders when a theme is imported.
 */

import type { ColorThemeDefinition } from './types'

let customThemes: readonly ColorThemeDefinition[] = []
let customThemesById = new Map<string, ColorThemeDefinition>()
let cacheVersion = 0
const cacheListeners = new Set<() => void>()

/** Replace the registry contents and notify subscribers. */
export function setCustomColorThemes(themes: readonly ColorThemeDefinition[]): void {
  customThemes = themes
  customThemesById = new Map(themes.map((theme) => [theme.id, theme]))
  cacheVersion += 1
  for (const listener of cacheListeners) {
    listener()
  }
}

/** Subscribe to registry updates (for `useSyncExternalStore` consumers). */
export function subscribeCustomColorThemes(listener: () => void): () => void {
  cacheListeners.add(listener)
  return () => {
    cacheListeners.delete(listener)
  }
}

export function getCustomColorThemesCacheVersion(): number {
  return cacheVersion
}

/**
 * The registered themes, in import order.
 *
 * The array identity is stable between {@link setCustomColorThemes} calls, so
 * it doubles as a `useSyncExternalStore` snapshot.
 */
export function getCustomColorThemes(): readonly ColorThemeDefinition[] {
  return customThemes
}

export function getCustomColorTheme(themeId: string): ColorThemeDefinition | undefined {
  return customThemesById.get(themeId)
}
