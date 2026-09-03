/**
 * T-H06 — the persisted `terminalUrlOpenMode` must survive the rename.
 *
 * The mode is read out of a persisted `app-settings` snapshot in
 * `src/__fixtures__/legacy-brand/` rather than inlined, because an inline
 * `'termul'` is a copy of the branch literal in `openTerminalUrl`: one
 * repo-wide sed rewrites the assertion and the comparison together, the suite
 * stays green, and every user who chose the built-in browser is silently moved
 * back to the system one. The fixture is sha256-frozen
 * (`src/__fixtures__/legacy-brand-manifest.test.ts`), so the two sides cannot
 * move together.
 *
 * The branch decision is never re-implemented here — the real `openTerminalUrl`
 * makes it, and the observable difference is which of the two sinks ran.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import {
  __resetBrandCanonicalOverride,
  __setBrandCanonicalOverride,
  brandCanonical,
  LEGACY
} from '@shared/brand'
import { afterEach, beforeEach, describe, expect, it, test, vi } from 'vitest'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { useBrowserSessionStore } from '@/stores/browser-session-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import { DEFAULT_APP_SETTINGS, type TerminalUrlOpenMode } from '@/types/settings'

const { mockOpenUrlWithSystemBrowser } = vi.hoisted(() => ({
  mockOpenUrlWithSystemBrowser: vi.fn()
}))

vi.mock('@/lib/api', () => ({
  openerApi: {
    openUrlWithSystemBrowser: mockOpenUrlWithSystemBrowser
  }
}))

import { openTerminalUrl } from './terminal-url-navigation'

const FIXTURE = join(
  process.cwd(),
  'src/__fixtures__/legacy-brand/app-settings-url-open-mode-termul.json'
)

/** The `terminalUrlOpenMode` a pre-rename install wrote into its app settings. */
function persistedOpenMode(): string {
  const settings = JSON.parse(readFileSync(FIXTURE, 'utf8')) as { terminalUrlOpenMode: string }
  return settings.terminalUrlOpenMode
}

/**
 * Seed the real store with a persisted mode. The cast mirrors the neighbouring
 * suite's `'invalid-mode' as TerminalUrlOpenMode`: a value read back from disk
 * is a `string`, and after the flip the canonical member is not yet in the
 * union — modelling that is the whole point of the test.
 */
function loadPersistedMode(mode: string): void {
  useAppSettingsStore.setState({
    settings: { ...DEFAULT_APP_SETTINGS, terminalUrlOpenMode: mode as TerminalUrlOpenMode },
    isLoaded: true
  })
}

/** The built-in browser branch opened a tab and never touched the system one. */
function expectBuiltInBrowserBranch(url: string): void {
  expect(mockOpenUrlWithSystemBrowser).not.toHaveBeenCalled()
  const tabs = Array.from(useBrowserSessionStore.getState().tabs.values())
  expect(tabs).toHaveLength(1)
  expect(tabs[0]?.url).toBe(url)
}

beforeEach(() => {
  vi.clearAllMocks()
  useAppSettingsStore.setState({ settings: { ...DEFAULT_APP_SETTINGS }, isLoaded: true })
  useBrowserSessionStore.setState({ tabs: new Map() })
  useWorkspaceStore.setState(() => ({
    root: { type: 'leaf', id: 'pane-root', tabs: [], activeTabId: null },
    activePaneId: 'pane-root',
    fullscreenPaneId: null
  }))
  mockOpenUrlWithSystemBrowser.mockResolvedValue({ success: true, data: undefined })
})

afterEach(() => {
  __resetBrandCanonicalOverride()
})

describe('terminal url open mode across the rename', () => {
  it('still honours the mode a pre-rename install persisted', async () => {
    // Green today, and that is the point: it goes red the moment the branch
    // literal is renamed without a compatibility read behind it.
    __setBrandCanonicalOverride({ urlOpenMode: 'se' })
    expect(persistedOpenMode()).toBe(LEGACY.urlOpenMode)
    loadPersistedMode(persistedOpenMode())

    await openTerminalUrl('https://example.com')

    expectBuiltInBrowserBranch('https://example.com')
  })

  // LEDGER (Wave 4) — expected failure. `openTerminalUrl` compares the
  // persisted mode against a hardcoded 'termul' and consults neither
  // `brandCanonical()` nor `LEGACY`, so the mode written *after* the flip falls
  // through to the system browser. Delete this test, `.fails` and all, once the
  // branch accepts the canonical mode alongside the legacy one.
  test.fails('honours the post-rename mode', async () => {
    __setBrandCanonicalOverride({ urlOpenMode: 'se' })
    loadPersistedMode(brandCanonical().urlOpenMode)

    await openTerminalUrl('https://example.com')

    expectBuiltInBrowserBranch('https://example.com')
  })
})
