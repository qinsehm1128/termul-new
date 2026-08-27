import type { IpcResult } from '@shared/types/ipc.types'
import { renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { useCliSessionPanelStore } from '@/stores/cli-session-panel-store'
import { useFileExplorerStore } from '@/stores/file-explorer-store'
import { useSidebarStore } from '@/stores/sidebar-store'
import { useTerminalListPanelStore } from '@/stores/terminal-list-panel-store'
import { APP_SETTINGS_KEY, DEFAULT_APP_SETTINGS } from '@/types/settings'
import {
  resetAppSettingsPersistenceQueueForTests,
  useAppSettingsLoader,
  useResetAppSettings,
  useUpdateAppSetting,
  useUpdateAppSettings,
  useUpdatePanelVisibility,
  waitForPendingAppSettingsPersistence
} from './use-app-settings'

const {
  mockPersistenceRead,
  mockPersistenceWrite,
  mockPersistenceWriteDebounced,
  mockUpdateOrphanDetection,
  mockSetTurnTimeout,
  mockSetTurnIdleTimeout,
  mockSetSessionNewTimeout,
  mockSetSessionReopenTimeout,
  mockSetFirstPromptWarmupTimeout,
  mockSetPreferLocalNpmInstall
} = vi.hoisted(() => ({
  mockPersistenceRead: vi.fn(),
  mockPersistenceWrite: vi.fn(),
  mockPersistenceWriteDebounced: vi.fn(),
  mockUpdateOrphanDetection: vi.fn(),
  mockSetTurnTimeout: vi.fn(),
  mockSetTurnIdleTimeout: vi.fn(),
  mockSetSessionNewTimeout: vi.fn(),
  mockSetSessionReopenTimeout: vi.fn(),
  mockSetFirstPromptWarmupTimeout: vi.fn(),
  mockSetPreferLocalNpmInstall: vi.fn()
}))

vi.mock('@/lib/api', () => ({
  acpApi: {
    setTurnTimeout: mockSetTurnTimeout,
    setTurnIdleTimeout: mockSetTurnIdleTimeout,
    setSessionNewTimeout: mockSetSessionNewTimeout,
    setSessionReopenTimeout: mockSetSessionReopenTimeout,
    setFirstPromptWarmupTimeout: mockSetFirstPromptWarmupTimeout,
    setPreferLocalNpmInstall: mockSetPreferLocalNpmInstall
  },
  persistenceApi: {
    read: mockPersistenceRead,
    write: mockPersistenceWrite,
    writeDebounced: mockPersistenceWriteDebounced
  },
  terminalApi: {
    updateOrphanDetection: mockUpdateOrphanDetection
  }
}))

describe('use-app-settings', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    resetAppSettingsPersistenceQueueForTests()

    useAppSettingsStore.setState({
      settings: { ...DEFAULT_APP_SETTINGS },
      isLoaded: false
    })
    useSidebarStore.setState({ isVisible: true })
    useFileExplorerStore.setState({ isVisible: true })
    useTerminalListPanelStore.setState({ isVisible: false })

    mockPersistenceRead.mockResolvedValue({ success: true, data: null })
    mockPersistenceWrite.mockResolvedValue({ success: true, data: undefined })
    mockPersistenceWriteDebounced.mockResolvedValue({ success: true, data: undefined })
    mockUpdateOrphanDetection.mockResolvedValue({ success: true, data: undefined })
    mockSetTurnTimeout.mockResolvedValue(undefined)
    mockSetTurnIdleTimeout.mockResolvedValue(undefined)
    mockSetSessionNewTimeout.mockResolvedValue(undefined)
    mockSetSessionReopenTimeout.mockResolvedValue(undefined)
    mockSetFirstPromptWarmupTimeout.mockResolvedValue(undefined)
    mockSetPreferLocalNpmInstall.mockResolvedValue(undefined)
  })

  it('hydrates sidebar and file explorer visibility from persisted app settings', async () => {
    mockPersistenceRead.mockResolvedValueOnce({
      success: true,
      data: {
        ...DEFAULT_APP_SETTINGS,
        sidebarVisible: false,
        fileExplorerVisible: true
      }
    })

    renderHook(() => useAppSettingsLoader())

    await waitFor(() => {
      expect(useAppSettingsStore.getState().isLoaded).toBe(true)
      expect(useSidebarStore.getState().isVisible).toBe(false)
      expect(useFileExplorerStore.getState().isVisible).toBe(true)
    })
  })

  it('pushes the ACP timeout overrides to the backend on load', async () => {
    mockPersistenceRead.mockResolvedValueOnce({
      success: true,
      data: {
        ...DEFAULT_APP_SETTINGS,
        acpTurnTimeoutSecs: 7200,
        acpTurnIdleTimeoutSecs: 1800,
        acpSessionNewTimeoutSecs: 120,
        acpSessionReopenTimeoutSecs: 300,
        acpFirstPromptWarmupSecs: 0,
        acpPreferLocalNpmInstall: false
      }
    })

    renderHook(() => useAppSettingsLoader())

    await waitFor(() => {
      expect(useAppSettingsStore.getState().isLoaded).toBe(true)
      expect(mockSetTurnTimeout).toHaveBeenCalledWith(7200)
      expect(mockSetTurnIdleTimeout).toHaveBeenCalledWith(1800)
      expect(mockSetSessionNewTimeout).toHaveBeenCalledWith(120)
      expect(mockSetSessionReopenTimeout).toHaveBeenCalledWith(300)
      expect(mockSetFirstPromptWarmupTimeout).toHaveBeenCalledWith(0)
      expect(mockSetPreferLocalNpmInstall).toHaveBeenCalledWith(false)
    })
  })

  it('defaults terminal URL open mode when persisted settings are missing the key', async () => {
    const { terminalUrlOpenMode: _terminalUrlOpenMode, ...legacySettings } = DEFAULT_APP_SETTINGS
    mockPersistenceRead.mockResolvedValueOnce({
      success: true,
      data: legacySettings
    })

    renderHook(() => useAppSettingsLoader())

    await waitFor(() => {
      expect(useAppSettingsStore.getState().isLoaded).toBe(true)
      expect(useAppSettingsStore.getState().settings.terminalUrlOpenMode).toBe('system')
    })
  })

  it('keeps screen reader mode disabled for legacy persisted settings', async () => {
    const { terminalScreenReaderMode: _terminalScreenReaderMode, ...legacySettings } =
      DEFAULT_APP_SETTINGS
    mockPersistenceRead.mockResolvedValueOnce({
      success: true,
      data: legacySettings
    })

    renderHook(() => useAppSettingsLoader())

    await waitFor(() => {
      expect(useAppSettingsStore.getState().settings.terminalScreenReaderMode).toBe(false)
    })
  })

  it('persists normalized legacy light color theme settings', async () => {
    mockPersistenceRead.mockResolvedValueOnce({
      success: true,
      data: {
        ...DEFAULT_APP_SETTINGS,
        colorTheme: 'termul-light',
        appearanceMode: undefined
      }
    })

    renderHook(() => useAppSettingsLoader())

    await waitFor(() => {
      expect(mockPersistenceWriteDebounced).toHaveBeenCalledWith(
        APP_SETTINGS_KEY,
        expect.objectContaining({ colorTheme: 'termul', appearanceMode: 'light' })
      )
    })
  })

  it('persists missing appearance mode defaults', async () => {
    mockPersistenceRead.mockResolvedValueOnce({
      success: true,
      data: {
        ...DEFAULT_APP_SETTINGS,
        appearanceMode: undefined
      }
    })

    renderHook(() => useAppSettingsLoader())

    await waitFor(() => {
      expect(mockPersistenceWriteDebounced).toHaveBeenCalledWith(
        APP_SETTINGS_KEY,
        expect.objectContaining({ appearanceMode: 'dark' })
      )
    })
  })

  it('persists terminal renderer migration from canvas to dom', async () => {
    mockPersistenceRead.mockResolvedValueOnce({
      success: true,
      data: {
        ...DEFAULT_APP_SETTINGS,
        terminalRenderer: 'canvas'
      }
    })

    renderHook(() => useAppSettingsLoader())

    await waitFor(() => {
      expect(mockPersistenceWriteDebounced).toHaveBeenCalledWith(
        APP_SETTINGS_KEY,
        expect.objectContaining({ terminalRenderer: 'dom' })
      )
    })
  })

  it('updates panel visibility with immediate persistence write', async () => {
    const { result } = renderHook(() => useUpdatePanelVisibility())

    await result.current('sidebarVisible', false)

    expect(useAppSettingsStore.getState().settings.sidebarVisible).toBe(false)
    expect(useSidebarStore.getState().isVisible).toBe(false)
    expect(mockPersistenceWrite).toHaveBeenCalledWith(
      APP_SETTINGS_KEY,
      expect.objectContaining({ sidebarVisible: false })
    )
    expect(mockPersistenceWriteDebounced).not.toHaveBeenCalled()
  })

  it('serializes rapid panel writes and persists each queued request payload', async () => {
    const deferredResolvers: Array<(result: IpcResult<void>) => void> = []
    mockPersistenceWrite.mockImplementation(
      () =>
        new Promise<IpcResult<void>>((resolve) => {
          deferredResolvers.push(resolve)
        })
    )

    const { result } = renderHook(() => useUpdatePanelVisibility())

    const first = result.current('sidebarVisible', false)
    const second = result.current('sidebarVisible', true)

    await waitFor(() => {
      expect(mockPersistenceWrite).toHaveBeenCalledTimes(1)
    })

    expect(mockPersistenceWrite).toHaveBeenNthCalledWith(
      1,
      APP_SETTINGS_KEY,
      expect.objectContaining({ sidebarVisible: false })
    )

    deferredResolvers[0]?.({ success: true, data: undefined })
    await first

    await waitFor(() => {
      expect(mockPersistenceWrite).toHaveBeenCalledTimes(2)
    })

    expect(mockPersistenceWrite).toHaveBeenNthCalledWith(
      2,
      APP_SETTINGS_KEY,
      expect.objectContaining({ sidebarVisible: true })
    )

    deferredResolvers[1]?.({ success: true, data: undefined })
    await second

    expect(useAppSettingsStore.getState().settings.sidebarVisible).toBe(true)
    expect(useSidebarStore.getState().isVisible).toBe(true)
  })

  it('reverts panel visibility in stores when immediate persistence fails', async () => {
    mockPersistenceWrite.mockResolvedValueOnce({
      success: false,
      error: 'write failed',
      code: 'WRITE_FAILED'
    })

    const { result } = renderHook(() => useUpdatePanelVisibility())

    let thrown: unknown
    try {
      await result.current('sidebarVisible', false)
    } catch (error) {
      thrown = error
    }

    expect(thrown).toBeInstanceOf(Error)
    expect((thrown as Error).message).toBe('write failed')
    expect(useAppSettingsStore.getState().settings.sidebarVisible).toBe(true)
    expect(useSidebarStore.getState().isVisible).toBe(true)
  })

  it('keeps newer panel value when older queued write fails', async () => {
    mockPersistenceWrite
      .mockResolvedValueOnce({ success: false, error: 'first failed', code: 'WRITE_FAILED' })
      .mockResolvedValueOnce({ success: true, data: undefined })

    const { result } = renderHook(() => useUpdatePanelVisibility())

    let firstError: unknown
    const first = result.current('fileExplorerVisible', false).catch((error) => {
      firstError = error
    })
    const second = result.current('fileExplorerVisible', true)

    await first
    await second

    expect(firstError).toBeInstanceOf(Error)
    expect((firstError as Error).message).toBe('first failed')

    expect(mockPersistenceWrite).toHaveBeenNthCalledWith(
      1,
      APP_SETTINGS_KEY,
      expect.objectContaining({ fileExplorerVisible: false })
    )
    expect(mockPersistenceWrite).toHaveBeenNthCalledWith(
      2,
      APP_SETTINGS_KEY,
      expect.objectContaining({ fileExplorerVisible: true })
    )
    expect(useAppSettingsStore.getState().settings.fileExplorerVisible).toBe(true)
    expect(useFileExplorerStore.getState().isVisible).toBe(true)
  })

  it('keeps debounced writes for non-panel app settings', async () => {
    const { result } = renderHook(() => useUpdateAppSetting())

    await result.current('terminalFontSize', 16)

    expect(useAppSettingsStore.getState().settings.terminalFontSize).toBe(16)
    expect(mockPersistenceWriteDebounced).toHaveBeenCalledWith(
      APP_SETTINGS_KEY,
      expect.objectContaining({ terminalFontSize: 16 })
    )
  })

  it('writes one snapshot when updating multiple app settings', async () => {
    const { result } = renderHook(() => useUpdateAppSettings())

    await result.current({ colorTheme: 'dracula', appearanceMode: 'light' })

    expect(useAppSettingsStore.getState().settings.colorTheme).toBe('dracula')
    expect(useAppSettingsStore.getState().settings.appearanceMode).toBe('light')
    expect(mockPersistenceWriteDebounced).toHaveBeenCalledTimes(1)
    expect(mockPersistenceWriteDebounced).toHaveBeenCalledWith(
      APP_SETTINGS_KEY,
      expect.objectContaining({ colorTheme: 'dracula', appearanceMode: 'light' })
    )
  })

  it('resets panel stores when app settings are reset', async () => {
    useSidebarStore.setState({ isVisible: false })
    useFileExplorerStore.setState({ isVisible: false })
    useCliSessionPanelStore.setState({ isVisible: true })

    const { result } = renderHook(() => useResetAppSettings())

    await result.current()

    expect(useSidebarStore.getState().isVisible).toBe(DEFAULT_APP_SETTINGS.sidebarVisible)
    expect(useFileExplorerStore.getState().isVisible).toBe(DEFAULT_APP_SETTINGS.fileExplorerVisible)
    expect(useCliSessionPanelStore.getState().isVisible).toBe(
      DEFAULT_APP_SETTINGS.cliSessionPanelVisible
    )
    expect(mockPersistenceWrite).toHaveBeenCalledWith(APP_SETTINGS_KEY, DEFAULT_APP_SETTINGS)
  })

  it('clears all ACP timeout overrides in the backend when app settings are reset', async () => {
    useAppSettingsStore.setState({
      settings: {
        ...DEFAULT_APP_SETTINGS,
        acpTurnTimeoutSecs: 7200,
        acpTurnIdleTimeoutSecs: 1800,
        acpSessionNewTimeoutSecs: 120,
        acpSessionReopenTimeoutSecs: 300,
        acpFirstPromptWarmupSecs: 15
      },
      isLoaded: true
    })

    const { result } = renderHook(() => useResetAppSettings())

    await result.current()

    expect(mockSetTurnTimeout).toHaveBeenCalledWith(null)
    expect(mockSetTurnIdleTimeout).toHaveBeenCalledWith(null)
    expect(mockSetSessionNewTimeout).toHaveBeenCalledWith(null)
    expect(mockSetSessionReopenTimeout).toHaveBeenCalledWith(null)
    expect(mockSetFirstPromptWarmupTimeout).toHaveBeenCalledWith(null)
    expect(mockSetPreferLocalNpmInstall).toHaveBeenCalledWith(true)
  })

  it('waits for queued panel writes before close-flow synchronization', async () => {
    let resolveWrite: ((result: IpcResult<void>) => void) | undefined
    mockPersistenceWrite.mockImplementationOnce(
      () =>
        new Promise<IpcResult<void>>((resolve) => {
          resolveWrite = resolve
        })
    )

    const { result } = renderHook(() => useUpdatePanelVisibility())
    const pendingWrite = result.current('sidebarVisible', false)

    const waiter = waitForPendingAppSettingsPersistence()

    let waiterResolved = false
    void waiter.then(() => {
      waiterResolved = true
    })

    await Promise.resolve()
    expect(waiterResolved).toBe(false)

    resolveWrite?.({ success: true, data: undefined })
    await pendingWrite
    await waiter

    expect(waiterResolved).toBe(true)
  })

  it('does not let an older failed revision roll back a newer successful state', async () => {
    const deferredResolvers: Array<(result: IpcResult<void>) => void> = []
    mockPersistenceWrite.mockImplementation(
      () =>
        new Promise<IpcResult<void>>((resolve) => {
          deferredResolvers.push(resolve)
        })
    )

    const { result } = renderHook(() => useUpdatePanelVisibility())

    const first = result.current('sidebarVisible', false).catch((error) => error)
    const second = result.current('sidebarVisible', true)

    await waitFor(() => {
      expect(mockPersistenceWrite).toHaveBeenCalledTimes(1)
    })

    deferredResolvers[0]?.({ success: false, error: 'first failed', code: 'WRITE_FAILED' })
    await first

    await waitFor(() => {
      expect(mockPersistenceWrite).toHaveBeenCalledTimes(2)
    })

    deferredResolvers[1]?.({ success: true, data: undefined })
    await second

    expect(useAppSettingsStore.getState().settings.sidebarVisible).toBe(true)
    expect(useSidebarStore.getState().isVisible).toBe(true)
  })
  it('hydrates terminal list visibility from persisted app settings', async () => {
    mockPersistenceRead.mockResolvedValueOnce({
      success: true,
      data: { ...DEFAULT_APP_SETTINGS, terminalListPanelVisible: true }
    })

    renderHook(() => useAppSettingsLoader())

    // The whole point of the setting: the list is back after a restart.
    await waitFor(() => {
      expect(useTerminalListPanelStore.getState().isVisible).toBe(true)
    })
  })

  it('persists and applies a terminal list visibility toggle', async () => {
    const { result } = renderHook(() => useUpdatePanelVisibility())

    await result.current('terminalListPanelVisible', true)

    expect(useTerminalListPanelStore.getState().isVisible).toBe(true)
    expect(mockPersistenceWrite).toHaveBeenCalledWith(
      APP_SETTINGS_KEY,
      expect.objectContaining({ terminalListPanelVisible: true })
    )
  })

  it('does not disturb the file explorer when toggling the terminal list', async () => {
    // applyPanelVisibilityToUi used to fall through to the file explorer for
    // any key without its own branch, so a new panel silently toggled a
    // different one.
    // Start it at the opposite of the value being applied, so a misroute is
    // observable rather than coincidentally matching.
    useFileExplorerStore.setState({ isVisible: false })
    const { result } = renderHook(() => useUpdatePanelVisibility())

    await result.current('terminalListPanelVisible', true)

    expect(useFileExplorerStore.getState().isVisible).toBe(false)
  })

  it('reverts terminal list visibility when the persistence write fails', async () => {
    mockPersistenceWrite.mockResolvedValueOnce({ success: false, error: 'disk full' })
    const { result } = renderHook(() => useUpdatePanelVisibility())

    await expect(result.current('terminalListPanelVisible', true)).rejects.toThrow('disk full')

    expect(useTerminalListPanelStore.getState().isVisible).toBe(false)
    expect(useAppSettingsStore.getState().settings.terminalListPanelVisible).toBe(false)
  })

  it('writes the persisted terminal list value, not an unpersisted optimistic one', async () => {
    // buildPanelWriteSnapshot rebuilds every panel field from the persisted
    // snapshot precisely so one panel's write cannot smuggle another panel's
    // never-persisted store value into the payload. A key missing from that
    // list falls back to the optimistic value and does exactly that.
    useAppSettingsStore.setState({
      settings: { ...DEFAULT_APP_SETTINGS, terminalListPanelVisible: true }
    })
    const { result } = renderHook(() => useUpdatePanelVisibility())

    await result.current('sidebarVisible', false)

    expect(mockPersistenceWrite).toHaveBeenCalledWith(
      APP_SETTINGS_KEY,
      expect.objectContaining({ sidebarVisible: false, terminalListPanelVisible: false })
    )
  })

  it('resets the terminal list store when app settings are reset', async () => {
    useTerminalListPanelStore.setState({ isVisible: true })

    const { result } = renderHook(() => useResetAppSettings())

    await result.current()

    expect(useTerminalListPanelStore.getState().isVisible).toBe(
      DEFAULT_APP_SETTINGS.terminalListPanelVisible
    )
  })
})
