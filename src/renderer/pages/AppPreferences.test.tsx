import { brandCanonical, LEGACY } from '@shared/brand'
import type {
  PendingUpdatePlan,
  UpdateComponent,
  UpdateComponentAction,
  UpdateComponentPolicy
} from '@shared/types/updater.types'
import { legacyUpdateComponentPolicy } from '@shared/types/updater.types'
import { confirm } from '@tauri-apps/plugin-dialog'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
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
        frpPublicHttps: false,
        frpTokenSet: false,
        sshHost: null,
        sshPort: null,
        sshUser: null,
        sshRemotePort: null,
        sshPublicHostname: null,
        sshIdentityFile: null,
        sshPublicHttps: false,
        sshPrivateKeySet: false,
        sshPasswordSet: false
      }
    }),
    set: vi.fn(),
    listSshHosts: vi.fn().mockResolvedValue({ success: true, data: [] })
  }
}))

const { updaterFixture, updaterActions, isAurUpdateMode, hasActiveTerminalSessions } = vi.hoisted(
  () => ({
    updaterFixture: {
      isChecking: false,
      updateAvailable: false,
      downloaded: false,
      version: '0.4.8' as string | null,
      lastChecked: null as Date | null,
      autoUpdateEnabled: false,
      skippedVersion: null as string | null,
      error: null as string | null,
      isManualUpdateMode: false,
      updateChannel: 'stable' as const,
      componentPolicy: null as UpdateComponentPolicy | null,
      pendingUpdatePlan: null as PendingUpdatePlan | null,
      isDownloading: false,
      downloadProgress: 0
    },
    updaterActions: {
      checkForUpdates: vi.fn(),
      downloadUpdate: vi.fn(),
      installAndRestart: vi.fn(),
      forceInstallAndRestart: vi.fn(),
      setAutoUpdateEnabled: vi.fn(),
      setUpdateChannel: vi.fn()
    },
    isAurUpdateMode: vi.fn(() => false),
    hasActiveTerminalSessions: vi.fn(() => false)
  })
)

vi.mock('@/lib/tauri-updater-api', () => ({
  isAurUpdateMode: () => isAurUpdateMode()
}))

const { fetchComponentRuntimeIdentities } = vi.hoisted(() => ({
  fetchComponentRuntimeIdentities: vi.fn()
}))

vi.mock('@/lib/update-component-diff', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/update-component-diff')>()),
  fetchComponentRuntimeIdentities: () => fetchComponentRuntimeIdentities()
}))

vi.mock('@/lib/tauri-safe-update', () => ({
  hasActiveTerminalSessions: () => hasActiveTerminalSessions()
}))

vi.mock('@/stores/updater-store', () => ({
  useUpdaterState: () => updaterFixture,
  useUpdaterActions: () => updaterActions
}))

vi.mock('@/stores/keyboard-shortcuts-store', () => ({
  useKeyboardShortcutsStore: vi.fn((selector: (s: { shortcuts: unknown[] }) => unknown) =>
    selector({ shortcuts: [] })
  ),
  detectShortcutConflicts: () => ({})
}))

vi.mock('@/components/KeybindingSchemePicker', () => ({ KeybindingSchemePicker: () => null }))

const mockResetAllShortcuts = vi.fn().mockResolvedValue(undefined)
vi.mock('@/hooks/use-keyboard-shortcuts', () => ({
  useUpdateShortcut: () => vi.fn(),
  useResetShortcut: () => vi.fn(),
  useResetAllShortcuts: () => mockResetAllShortcuts
}))

vi.mock('@/components/settings/AcpAgentsSettings', () => ({
  AcpAgentsSettings: () => null
}))

vi.mock('@/components/settings/McpControlPanel', () => ({
  McpControlPanel: () => null
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

function policyWith(
  actions: Partial<Record<UpdateComponent, UpdateComponentAction>>
): UpdateComponentPolicy {
  const base = legacyUpdateComponentPolicy('1.2.3')
  const components = { ...base.components }
  for (const component of Object.keys(actions) as UpdateComponent[]) {
    const action = actions[component]
    if (!action) continue
    components[component] = { buildId: `build-${component}`, action }
  }
  return { ...base, metadataState: 'declared', components }
}

function pendingPlan(
  status: PendingUpdatePlan['status'],
  overrides: Partial<PendingUpdatePlan> = {}
): PendingUpdatePlan {
  return {
    schemaVersion: 1,
    targetVersion: '1.2.3',
    componentPolicy: legacyUpdateComponentPolicy('1.2.3'),
    status,
    currentComponentBuildIds: { acpCore: 'live-build-not-for-ui' },
    requiredActions: ['acpCore'],
    deferredComponents: [],
    createdAt: '2026-08-14T00:00:00.000Z',
    updatedAt: '2026-08-14T00:00:00.000Z',
    ...overrides
  }
}

describe('AppPreferences settings controls', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    isAurUpdateMode.mockReturnValue(false)
    hasActiveTerminalSessions.mockReturnValue(false)
    vi.mocked(confirm).mockResolvedValue(false)
    updaterFixture.isChecking = false
    updaterFixture.updateAvailable = false
    updaterFixture.downloaded = false
    updaterFixture.version = '0.4.8'
    updaterFixture.lastChecked = null
    updaterFixture.autoUpdateEnabled = false
    updaterFixture.skippedVersion = null
    updaterFixture.error = null
    updaterFixture.isManualUpdateMode = false
    updaterFixture.componentPolicy = null
    updaterFixture.pendingUpdatePlan = null
    updaterFixture.isDownloading = false
    updaterFixture.downloadProgress = 0
    useAppSettingsStore.setState({ settings: { ...DEFAULT_APP_SETTINGS }, isLoaded: true })
  })

  afterEach(() => {
    vi.mocked(confirm).mockReset()
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

  it('writes the dragged sidebar text size and text contrast', async () => {
    renderPage()

    const size = await screen.findByLabelText('Sidebar text size')
    expect(size).toHaveAttribute('type', 'range')
    fireEvent.change(size, { target: { value: '15' } })
    fireEvent.change(screen.getByLabelText('Text contrast'), { target: { value: '35' } })

    await waitFor(() => {
      const { settings } = useAppSettingsStore.getState()
      expect(settings.sidebarFontSize).toBe(15)
      expect(settings.uiContrast).toBe(35)
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

  it('resumes agent sessions on restore by default and lets the user turn it off', async () => {
    renderPage()

    const toggle = await screen.findByRole('switch', { name: 'Resume agent sessions on restore' })
    expect(toggle).toHaveAttribute('aria-checked', 'true')

    fireEvent.click(toggle)

    await waitFor(() => {
      expect(useAppSettingsStore.getState().settings.resumeAgentSessionsOnRestore).toBe(false)
    })
    expect(mockWriteDebounced).toHaveBeenCalled()
  })

  it('explains a preserved Core as reconnect and still restarts the GUI', () => {
    updaterFixture.updateAvailable = true
    updaterFixture.version = '1.2.3'
    updaterFixture.componentPolicy = policyWith({
      renderer: 'restart',
      guiNative: 'restart',
      acpCore: 'preserve',
      terminalCore: 'preserve'
    })

    renderPage()

    expect(screen.getByText(/The app window always restarts/)).toBeInTheDocument()
    expect(
      screen.getByText('Kept and reconnected because the build matches: ACP Core, Terminal Core.')
    ).toBeInTheDocument()
    expect(screen.queryByText(/ACP Core does not match/)).not.toBeInTheDocument()
    expect(screen.queryByText(/Terminal Core does not match/)).not.toBeInTheDocument()
    expect(screen.getByText(/Renderer restarts with the app window/)).toBeInTheDocument()
  })

  it('confirms active matching Cores separately from an active mismatched Terminal defer', async () => {
    updaterFixture.downloaded = true
    updaterFixture.version = '1.2.3'
    hasActiveTerminalSessions.mockReturnValue(true)
    updaterFixture.componentPolicy = policyWith({
      acpCore: 'preserve',
      terminalCore: 'preserve'
    })
    renderPage()

    fireEvent.click(screen.getByRole('button', { name: 'Safe Install & Restart' }))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    const matchingMessage = vi.mocked(confirm).mock.calls[0]?.[0] as string
    expect(matchingMessage).toContain('The app window always restarts.')
    expect(matchingMessage).toContain(
      'Restarting the app window itself does not stop terminals owned by Terminal Core.'
    )
    expect(matchingMessage).toContain(
      'Kept and reconnected because the build matches: ACP Core, Terminal Core.'
    )
    expect(matchingMessage).toContain('This is not a zero-interruption swap.')
    expect(matchingMessage).not.toContain('Replacement waits while a terminal is active.')
    expect(updaterActions.installAndRestart).not.toHaveBeenCalled()

    vi.mocked(confirm).mockClear()
    updaterFixture.componentPolicy = policyWith({
      acpCore: 'preserve',
      terminalCore: 'defer-if-active'
    })
    renderPage()
    fireEvent.click(screen.getAllByRole('button', { name: 'Safe Install & Restart' }).at(-1)!)
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    const deferredMessage = vi.mocked(confirm).mock.calls[0]?.[0] as string
    expect(deferredMessage).toContain(
      'Terminal Core build identity is checked. If it differs, replacement waits while a terminal is active.'
    )
    expect(deferredMessage).toContain('Kept and reconnected because the build matches: ACP Core.')
    expect(deferredMessage).not.toContain('No terminal is active')
    expect(updaterActions.installAndRestart).not.toHaveBeenCalled()
  })

  it('shows durable plan states, the failed error, and a check that does not install', () => {
    updaterFixture.pendingUpdatePlan = pendingPlan('failed', { lastError: 'reconnect timed out' })
    const view = renderPage()

    expect(
      screen.getByText('Saved update plan failed. Review the error, then check again.')
    ).toBeInTheDocument()
    expect(screen.getByText('reconnect timed out')).toBeInTheDocument()
    expect(screen.queryByText('live-build-not-for-ui')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }))
    expect(updaterActions.checkForUpdates).toHaveBeenCalledTimes(1)
    expect(updaterActions.installAndRestart).not.toHaveBeenCalled()

    updaterFixture.pendingUpdatePlan = pendingPlan('reconciling')
    view.rerender(
      <MemoryRouter>
        <AppPreferences />
      </MemoryRouter>
    )
    expect(
      screen.getByText(
        'Saved update plan: components are reconnecting. This is not a zero-interruption swap.'
      )
    ).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Check again' })).not.toBeInTheDocument()

    updaterFixture.pendingUpdatePlan = pendingPlan('deferred', {
      deferredComponents: ['terminalCore']
    })
    view.rerender(
      <MemoryRouter>
        <AppPreferences />
      </MemoryRouter>
    )
    expect(
      screen.getByText(
        'Saved update plan: replacement of Terminal Core is waiting because a terminal or AI session is still running. Restarting the app window does not interrupt them; to apply it now, use “Quit Se and End All Terminals” and reopen the app.'
      )
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Check again' })).toBeInTheDocument()

    updaterFixture.pendingUpdatePlan = pendingPlan('completed')
    view.rerender(
      <MemoryRouter>
        <AppPreferences />
      </MemoryRouter>
    )
    expect(
      screen.getByText(
        'Saved update plan finished. Matching Cores stayed running and were reconnected.'
      )
    ).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Check again' })).not.toBeInTheDocument()

    updaterFixture.pendingUpdatePlan = pendingPlan('prepared')
    view.rerender(
      <MemoryRouter>
        <AppPreferences />
      </MemoryRouter>
    )
    expect(screen.queryByText('Pending component migration')).not.toBeInTheDocument()
  })

  it('keeps the AUR and manual update instructions unchanged', () => {
    updaterFixture.updateAvailable = true
    updaterFixture.version = '1.2.3'
    isAurUpdateMode.mockReturnValue(true)
    renderPage()

    expect(screen.getByText('Update through AUR with: yay -S se-manager')).toBeInTheDocument()
    expect(screen.queryByText('Release Channel')).not.toBeInTheDocument()

    isAurUpdateMode.mockReturnValue(false)
    updaterFixture.isManualUpdateMode = true
    renderPage()
    expect(
      screen.getByText(
        'Automatic update is unavailable. Please download and install the latest version manually.'
      )
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Open Download Page' })).toBeInTheDocument()
  })
})

describe('AppPreferences update component diff and forced install', () => {
  const tauriWindow = window as unknown as Record<string, unknown>

  beforeEach(() => {
    vi.clearAllMocks()
    tauriWindow.__TAURI_INTERNALS__ = {}
    isAurUpdateMode.mockReturnValue(false)
    hasActiveTerminalSessions.mockReturnValue(true)
    vi.mocked(confirm).mockResolvedValue(false)
    updaterFixture.updateAvailable = true
    updaterFixture.downloaded = true
    updaterFixture.version = '1.2.3'
    updaterFixture.isManualUpdateMode = false
    updaterFixture.pendingUpdatePlan = null
    updaterFixture.isDownloading = false
    updaterFixture.downloadProgress = 0
    updaterFixture.componentPolicy = policyWith({
      renderer: 'restart',
      guiNative: 'restart',
      acpCore: 'defer-if-active',
      terminalCore: 'defer-if-active'
    })
    fetchComponentRuntimeIdentities.mockResolvedValue({
      guiNative: 'build-guiNative',
      acpCore: { buildId: 'build-acpCore', activeResources: 2 },
      terminalCore: { buildId: 'old-terminal', activeResources: 3 }
    })
    useAppSettingsStore.setState({ settings: { ...DEFAULT_APP_SETTINGS }, isLoaded: true })
  })

  afterEach(() => {
    delete tauriWindow.__TAURI_INTERNALS__
    vi.mocked(confirm).mockReset()
  })

  it('lists each component with its running and new build ID', async () => {
    renderPage()

    const table = await screen.findByTestId('update-component-diff')
    const status = (component: string) =>
      table.querySelector(`tr[data-component="${component}"]`)?.getAttribute('data-status')
    expect(status('renderer')).toBe('bundled')
    expect(status('guiNative')).toBe('same')
    expect(status('acpCore')).toBe('same')
    expect(status('terminalCore')).toBe('replace')
    expect(table).toHaveTextContent('old-terminal')
    expect(table).toHaveTextContent('3 terminal(s) running')
  })

  it('names the terminals a forced install ends and installs only after confirmation', async () => {
    renderPage()

    fireEvent.click(screen.getByRole('button', { name: 'Force Update' }))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    const message = vi.mocked(confirm).mock.calls[0]?.[0] as string
    expect(message).toContain('This ends 3 running terminal(s) and 0 agent session(s).')
    expect(updaterActions.forceInstallAndRestart).not.toHaveBeenCalled()

    vi.mocked(confirm).mockResolvedValue(true)
    fireEvent.click(screen.getByRole('button', { name: 'Force Update' }))
    await waitFor(() => expect(updaterActions.forceInstallAndRestart).toHaveBeenCalledTimes(1))
    expect(updaterActions.installAndRestart).not.toHaveBeenCalled()
  })

  it('offers a download with progress before the update is downloaded', () => {
    updaterFixture.downloaded = false
    updaterFixture.isDownloading = true
    updaterFixture.downloadProgress = 42

    renderPage()

    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '42')
    expect(screen.queryByRole('button', { name: 'Force Update' })).not.toBeInTheDocument()
    expect(screen.queryByText('Component changes')).not.toBeInTheDocument()
    expect(fetchComponentRuntimeIdentities).not.toHaveBeenCalled()
  })
})
