import { brandCanonical } from '@shared/brand'
import { describe, expect, it } from 'vitest'
import { TERMINAL_URL_OPEN_MODE_OPTIONS } from '@/types/settings'
import { resources } from './resources'

type JsonObject = Record<string, unknown>

function leafKeys(value: JsonObject, prefix = ''): string[] {
  return Object.entries(value).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key
    if (child && typeof child === 'object' && !Array.isArray(child)) {
      return leafKeys(child as JsonObject, path)
    }
    return [path]
  })
}

describe('translation catalogs', () => {
  it('keeps English and Simplified Chinese namespace key sets aligned', () => {
    for (const namespace of Object.keys(resources.en) as Array<keyof typeof resources.en>) {
      expect(leafKeys(resources['zh-CN'][namespace])).toEqual(leafKeys(resources.en[namespace]))
    }
  })

  /**
   * `AppPreferences` renders the URL-mode dropdown as
   * `tSettings(`options.urlMode.${option.value}`)`. That `t` carries no
   * `defaultValue` and i18next runs with `returnNull: false`, so a key the
   * catalog is missing renders as the key itself — the dropdown would read
   * "options.urlMode.se". Renaming the enum member without renaming the key is
   * therefore silent in every type check and in every other test.
   */
  it('carries a urlMode label for every terminal URL open mode', () => {
    for (const language of Object.keys(resources) as Array<keyof typeof resources>) {
      const labels = (resources[language].settings.options as { urlMode: Record<string, string> })
        .urlMode
      for (const option of TERMINAL_URL_OPEN_MODE_OPTIONS) {
        expect(Object.keys(labels)).toContain(option.value)
      }
      expect(Object.keys(labels).sort()).toEqual(
        TERMINAL_URL_OPEN_MODE_OPTIONS.map((option) => option.value).sort()
      )
    }
  })

  it('keeps the built-in-browser option spelled as the brand seam names it', () => {
    // The union type needs an authored literal, so this member is the one
    // canonical brand value that cannot be read from `brand.ts` at runtime.
    // This is the gate that catches the two drifting apart.
    expect(TERMINAL_URL_OPEN_MODE_OPTIONS.map((option) => option.value)).toContain(
      brandCanonical().urlOpenMode
    )
  })

  it('does not contain empty translations', () => {
    for (const language of Object.values(resources)) {
      for (const namespace of Object.values(language)) {
        for (const key of leafKeys(namespace)) {
          const value = key.split('.').reduce<unknown>((current, part) => {
            return (current as JsonObject)[part]
          }, namespace)
          expect(value).not.toBe('')
        }
      }
    }
  })
})
