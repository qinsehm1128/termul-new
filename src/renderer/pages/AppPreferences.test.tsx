import { brandCanonical, LEGACY } from '@shared/brand'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { DEFAULT_APP_SETTINGS, type TerminalUrlOpenMode } from '@/types/settings'
import AppPreferences from './AppPreferences'

/**
 * GH-539: the App Preferences switch/select are the ONLY write path for the
 * auto-save settings. These tests pin the wiring (click → store value), which
 * no auto-save unit test can reach because they inject settings directly.
 */

const mockWriteDebounced = vi.fn().mockResolvedValue(undefined)

vi.mock('@/lib/api', () => ({
  acpApi: { setTurnTimeout: vi.fn().mockResolvedValue({ success: true }) },
  logApi: {
    revealLogDir: vi.fn(),
    exportLogFile: vi.fn(),
    copyLogContents: vi.fn(),
    exportLogToDefault: vi.fn()
  },
  shellApi: {
    getAvailableShells: vi.fn().mockResolvedValue({
      success: true,
      data: { default: null, available: [] }
    })
  },
  terminalApi: { updateOrphanDetection: vi.fn().mockResolvedValue({ success: true }) },
  persistenceApi: {
    read: vi.fn().mockResolvedValue({ success: true, data: null }),
    write: vi.fn(),
    writeDebounced: (...args: unknown[]) => mockWriteDebounced(...args)
  },
  tunnelConfigApi: {
    get: vi.fn().mockResolvedValue({
      success: true,
      data: {
        provider: 'cloudflareQuick',
        cloudflareNamedHostname: null,
        cloudflareNamedLocalPort: null,
        cloudflareNamedTokenSet: false,
        frpServerAddr: null,
        frpServerPort: null,
        frpCustomDomain: null,
        frpRemotePort: null,
        frpPublicHttps: true,
        frpTokenSet: false,
        sshHost: null,
        sshPort: null,
        sshUser: null,
        sshRemotePort: null,
        sshPublicHostname: null,
        sshPublicHttps: true,
        sshPrivateKeySet: false
      }
    }),
    set: vi.fn()
  }
}))

vi.mock('@/lib/tauri-updater-api', () => ({
  isAurUpdateMode: () => false
}))

vi.mock('@/stores/updater-store', () => ({
  useUpdaterState: () => ({
    isChecking: false,
    updateAvailable: false,
    version: '0.4.8',
    lastChecked: null,
    autoUpdateEnabled: false,
    skippedVersion: null,
    error: null,
    isManualUpdateMode: false,
    updateChannel: 'stable'
  }),
  useUpdaterActions: () => ({
    checkForUpdates: vi.fn(),
    installAndRestart: vi.fn(),
    setAutoUpdateEnabled: vi.fn(),
    setUpdateChannel: vi.fn()
  })
}))

vi.mock('@/stores/keyboard-shortcuts-store', () => ({
  useKeyboardShortcutsStore: vi.fn((selector: (s: { shortcuts: unknown[] }) => unknown) =>
    selector({ shortcuts: [] })
  )
}))

const mockResetAllShortcuts = vi.fn().mockResolvedValue(undefined)
vi.mock('@/hooks/use-keyboard-shortcuts', () => ({
  useUpdateShortcut: () => vi.fn(),
  useResetShortcut: () => vi.fn(),
  useResetAllShortcuts: () => mockResetAllShortcuts
}))

vi.mock('@/components/settings/AcpAgentsSettings', () => ({
  AcpAgentsSettings: () => null
}))

vi.mock('@/components/settings/McpServersSettings', () => ({
  McpServersSettings: () => null
}))

vi.mock('@/components/settings/CliResumeDefaultsSettings', () => ({
  CliResumeDefaultsSettings: () => null
}))

function renderPage(): ReturnType<typeof render> {
  return render(
    <MemoryRouter>
      <AppPreferences />
    </MemoryRouter>
  )
}

describe('AppPreferences settings controls', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useAppSettingsStore.setState({ settings: { ...DEFAULT_APP_SETTINGS }, isLoaded: true })
  })

  it('toggling the auto-save switch writes editorAutoSave (with correct negation)', async () => {
    renderPage()

    const toggle = await screen.findByRole('switch', { name: 'Enable auto save' })
    expect(toggle).toHaveAttribute('aria-checked', 'false')

    fireEvent.click(toggle)
    await waitFor(() => {
      expect(useAppSettingsStore.getState().settings.editorAutoSave).toBe(true)
    })
    expect(mockWriteDebounced).toHaveBeenCalled()

    fireEvent.click(toggle)
    await waitFor(() => {
      expect(useAppSettingsStore.getState().settings.editorAutoSave).toBe(false)
    })
  })

  it('changing the delay select writes editorAutoSaveDelayMs and is disabled while off', async () => {
    renderPage()

    const select = await screen.findByLabelText('Auto save delay')
    expect(select).toBeDisabled()

    fireEvent.click(screen.getByRole('switch', { name: 'Enable auto save' }))
    await waitFor(() => {
      expect(screen.getByLabelText('Auto save delay')).toBeEnabled()
    })

    fireEvent.change(screen.getByLabelText('Auto save delay'), { target: { value: '2000' } })
    await waitFor(() => {
      expect(useAppSettingsStore.getState().settings.editorAutoSaveDelayMs).toBe(2000)
    })
  })

  /**
   * S-02 — the settings UI must not contradict `openTerminalUrl`.
   *
   * A blob written before the rename still names the built-in browser by its
   * legacy enum member. That member is absent from the option list (nothing
   * writes it any more), so a select bound to the raw value matches no
   * `<option>` and falls back to displaying the first entry. Meanwhile the
   * compatibility read in `openTerminalUrl` still routes links to the built-in
   * browser — the dropdown would state the opposite of what the app does.
   */
  it('shows the built-in browser for a mode persisted under the legacy id', async () => {
    useAppSettingsStore.setState({
      // Cast for the same reason as `terminal-url-navigation.brand.test.ts`:
      // a value read back from disk is a `string`, and the legacy member is
      // deliberately no longer part of the union.
      settings: {
        ...DEFAULT_APP_SETTINGS,
        terminalUrlOpenMode: LEGACY.urlOpenMode as TerminalUrlOpenMode
      },
      isLoaded: true
    })
    renderPage()

    const option = await screen.findByRole('option', { name: 'Se Browser' })
    const select = option.closest('select')
    expect(select).not.toBeNull()
    expect((select as HTMLSelectElement).value).toBe(brandCanonical().urlOpenMode)
    expect((select as HTMLSelectElement).selectedOptions[0]?.textContent).toBe('Se Browser')
  })

  it('leaves the persisted legacy mode on disk until the user picks something', async () => {
    useAppSettingsStore.setState({
      settings: {
        ...DEFAULT_APP_SETTINGS,
        terminalUrlOpenMode: LEGACY.urlOpenMode as TerminalUrlOpenMode
      },
      isLoaded: true
    })
    renderPage()

    await screen.findByRole('option', { name: 'Se Browser' })
    // Display normalization is a read. It must not schedule a write of its own.
    expect(useAppSettingsStore.getState().settings.terminalUrlOpenMode).toBe(LEGACY.urlOpenMode)
  })

  it('uses compact sidebar chrome for the page header', () => {
    renderPage()

    const header = screen.getByRole('heading', { name: 'Application Preferences' }).closest('.h-9')
    expect(header).toHaveClass('h-9', 'bg-sidebar')
  })

  it('keeps screen reader mode opt-in and persists the user toggle', async () => {
    renderPage()

    const toggle = await screen.findByRole('switch', { name: 'Screen reader mode' })
    expect(toggle).toHaveAttribute('aria-checked', 'false')

    fireEvent.click(toggle)

    await waitFor(() => {
      expect(useAppSettingsStore.getState().settings.terminalScreenReaderMode).toBe(true)
    })
    expect(mockWriteDebounced).toHaveBeenCalled()
  })
})
