import { legacyUpdateComponentPolicy } from '@shared/types/updater.types'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null
  }
}))

vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn()
}))

vi.mock('@tauri-apps/plugin-updater', () => ({
  check: vi.fn(),
  Update: class {}
}))

vi.mock('../app-restart', () => ({
  restartApp: vi.fn()
}))

vi.mock('../tauri-backup-api', () => ({
  BackupErrorCodes: {
    BACKUP_FAILED: 'BACKUP_FAILED',
    RESTORE_FAILED: 'RESTORE_FAILED',
    BACKUP_NOT_FOUND: 'BACKUP_NOT_FOUND',
    DISK_SPACE_ERROR: 'DISK_SPACE_ERROR',
    INVALID_BACKUP: 'INVALID_BACKUP'
  },
  createBackup: vi.fn(),
  setAppVersion: vi.fn()
}))

vi.mock('../tauri-rollback-api', () => ({
  keepPreviousVersion: vi.fn(),
  setCurrentVersion: vi.fn()
}))

import { getVersion } from '@tauri-apps/api/app'
import { invoke } from '@tauri-apps/api/core'
import { check } from '@tauri-apps/plugin-updater'
import { restartApp as relaunch } from '../app-restart'
import { createBackup, setAppVersion } from '../tauri-backup-api'
import { keepPreviousVersion, setCurrentVersion } from '../tauri-rollback-api'
import {
  _resetUpdaterStateForTesting,
  checkForUpdates,
  clearPendingUpdate,
  downloadUpdate,
  getAutoUpdateEnabled,
  getUpdaterState,
  installAndRestart,
  isUpdateAvailable,
  mapTauriUpdateToInfo,
  registerUpdateEventHandlers,
  setAutoUpdateEnabled,
  type UpdaterDownloadEvent
} from '../tauri-updater-api'

function createMockUpdate(version: string, body?: string, date?: string) {
  return {
    version,
    body,
    date,
    download: vi.fn(),
    install: vi.fn(),
    downloadAndInstall: vi.fn()
  }
}

interface RustUpdaterHandlers {
  download?: (emit: (event: UpdaterDownloadEvent) => void) => unknown
  install?: () => unknown
}

/** Route the Rust `updater_download` / `updater_install` commands. */
function mockRustUpdater(handlers: RustUpdaterHandlers = {}): void {
  vi.mocked(invoke).mockImplementation((async (command: string, args?: unknown) => {
    if (command === 'updater_download') {
      const channel = (args as { onEvent: { onmessage?: (event: UpdaterDownloadEvent) => void } })
        .onEvent
      return handlers.download
        ? await handlers.download((event) => channel.onmessage?.(event))
        : { success: true }
    }
    if (command === 'updater_install') {
      return handlers.install ? await handlers.install() : { success: true }
    }
    return undefined
  }) as never)
}

function invokeCalls(command: string): unknown[][] {
  return vi.mocked(invoke).mock.calls.filter((call) => call[0] === command)
}

function invokeOrder(command: string): number {
  const index = vi.mocked(invoke).mock.calls.findIndex((call) => call[0] === command)
  return vi.mocked(invoke).mock.invocationCallOrder[index]
}

async function flushPromises(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
}

const mockFetch = vi.fn()

describe('tauri-updater-api', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(invoke).mockReset()
    _resetUpdaterStateForTesting()
    vi.mocked(getVersion).mockResolvedValue('0.2.3')
    vi.mocked(setAppVersion).mockResolvedValue(undefined)
    vi.mocked(setCurrentVersion).mockResolvedValue(undefined)
    vi.mocked(createBackup).mockResolvedValue({
      success: true,
      data: {
        id: 'backup-1',
        timestamp: '2026-03-01T00:00:00.000Z',
        version: '0.2.3',
        size: 1024,
        fileCount: 4,
        path: '/mock/backups/backup-1'
      }
    } as never)
    vi.mocked(keepPreviousVersion).mockResolvedValue({
      success: true,
      data: {
        version: '0.2.3',
        path: '/mock/versions/v0.2.3',
        size: 2048
      }
    } as never)
    mockFetch.mockReset()
    vi.stubGlobal('fetch', mockFetch)
  })

  describe('checkForUpdates', () => {
    it('returns null when no update is available', async () => {
      vi.mocked(check).mockResolvedValue(null)

      const result = await checkForUpdates()

      expect(result).toBeNull()
      const state = await getUpdaterState()
      expect(state.success).toBe(true)
      if (state.success) {
        expect(state.data.updateAvailable).toBe(false)
        expect(state.data.version).toBeNull()
      }
    })

    it('returns mapped update info when update exists', async () => {
      const update = createMockUpdate('2.0.0', 'release notes', '2026-03-01T00:00:00.000Z')
      vi.mocked(check).mockResolvedValue(update as never)

      const result = await checkForUpdates()

      expect(result).toEqual({
        version: '2.0.0',
        releaseDate: '2026-03-01T00:00:00.000Z',
        releaseNotes: 'release notes',
        componentPolicy: legacyUpdateComponentPolicy('2.0.0'),
        isSecurityUpdate: false
      })
      expect(result?.componentPolicy?.components.pluginCore).toEqual({
        buildId: 'legacy:2.0.0:pluginCore',
        action: 'unsupported'
      })
    })

    it('throws actionable error details when check fails', async () => {
      vi.mocked(check).mockRejectedValue(new Error('network down'))

      await expect(checkForUpdates()).rejects.toThrow(
        'Failed to check for updates from https://github.com/qinsehm1128/termul-new/releases/latest/download/latest.json: network down'
      )
    })

    it('falls back to GitHub release metadata for missing-manifest style failures', async () => {
      vi.mocked(check).mockRejectedValue({ status: 404, url: 'latest.json' })
      mockFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          tag_name: 'v0.3.4',
          body: 'release notes',
          html_url: 'https://github.com/qinsehm1128/termul-new/releases/tag/v0.3.4',
          published_at: '2026-05-01T15:13:12Z'
        })
      })

      await expect(checkForUpdates()).resolves.toEqual({
        version: '0.3.4',
        releaseDate: '2026-05-01T15:13:12Z',
        releaseNotes: 'release notes',
        componentPolicy: legacyUpdateComponentPolicy('0.3.4'),
        isSecurityUpdate: false,
        downloadUrl: 'https://github.com/qinsehm1128/termul-new/releases/tag/v0.3.4'
      })

      const state = await getUpdaterState()
      expect(state.success).toBe(true)
      if (state.success) {
        expect(state.data.updateAvailable).toBe(true)
        expect(state.data.version).toBe('0.3.4')
        expect(state.data.isManualUpdateMode).toBe(true)
      }
    })

    it('preserves non-fallback error details when the manifest error is not eligible for GitHub fallback', async () => {
      vi.mocked(check).mockRejectedValue({ status: 500, url: 'latest.json' })

      await expect(checkForUpdates()).rejects.toThrow(
        'Failed to check for updates from https://github.com/qinsehm1128/termul-new/releases/latest/download/latest.json: {"status":500,"url":"latest.json"}'
      )
      expect(mockFetch).not.toHaveBeenCalled()
    })
  })

  describe('downloadUpdate', () => {
    it('returns UPDATE_NOT_AVAILABLE when no pending update exists', async () => {
      const result = await downloadUpdate()

      expect(result).toEqual({
        success: false,
        error: 'No update available to download',
        code: 'UPDATE_NOT_AVAILABLE'
      })
    })

    it('downloads through Rust with throttled totals and marks the version downloaded', async () => {
      const update = createMockUpdate('2.0.1', 'notes')
      vi.mocked(check).mockResolvedValue(update as never)
      await checkForUpdates()

      mockRustUpdater({
        download: (emit) => {
          emit({ event: 'Started', data: { contentLength: 100 } })
          emit({ event: 'Progress', data: { downloaded: 40, contentLength: 100 } })
          emit({ event: 'Progress', data: { downloaded: 100, contentLength: 100 } })
          emit({ event: 'Finished' })
          return { success: true }
        }
      })

      const progressEvents: number[] = []
      const result = await downloadUpdate((progress) => {
        progressEvents.push(progress.percent)
      })

      expect(result).toEqual({ success: true, data: undefined })
      // The package is fetched in Rust: the plugin's per-chunk JS channel is
      // what flooded the main thread and froze the window.
      expect(update.download).not.toHaveBeenCalled()
      expect(invokeCalls('updater_download')).toEqual([
        ['updater_download', { version: '2.0.1', onEvent: expect.anything() }]
      ])
      // No copy of userData: nothing could restore it, and it held the
      // download for minutes on a large profile.
      expect(createBackup).not.toHaveBeenCalled()
      expect(keepPreviousVersion).toHaveBeenCalledWith('0.2.3')
      // Download must not install/restart the app on its own.
      expect(invokeCalls('updater_install')).toHaveLength(0)
      expect(vi.mocked(keepPreviousVersion).mock.invocationCallOrder[0]).toBeLessThan(
        invokeOrder('updater_download')
      )
      expect(progressEvents[0]).toBe(0)
      expect(progressEvents).toContain(40)
      expect(progressEvents).toContain(100)

      const state = await getUpdaterState()
      expect(state.success).toBe(true)
      if (state.success) {
        expect(state.data.downloaded).toBe(true)
        expect(state.data.version).toBe('2.0.1')
      }
    })

    it('returns DOWNLOAD_FAILED when download throws', async () => {
      const update = createMockUpdate('2.0.2')
      vi.mocked(check).mockResolvedValue(update as never)
      await checkForUpdates()

      mockRustUpdater({
        download: () => {
          throw new Error('download failed')
        }
      })

      const result = await downloadUpdate()

      expect(result).toEqual({
        success: false,
        error: 'download failed',
        code: 'DOWNLOAD_FAILED'
      })
    })

    it('passes the Rust error code through and does not mark the update downloaded', async () => {
      const update = createMockUpdate('2.0.3')
      vi.mocked(check).mockResolvedValue(update as never)
      await checkForUpdates()

      mockRustUpdater({
        download: () => ({
          success: false,
          error: 'Update 2.0.3 is no longer the published version',
          code: 'UPDATE_NOT_AVAILABLE'
        })
      })

      const result = await downloadUpdate()

      expect(result).toEqual({
        success: false,
        error: 'Update 2.0.3 is no longer the published version',
        code: 'UPDATE_NOT_AVAILABLE'
      })
      expect(await installAndRestart()).toMatchObject({ code: 'UPDATE_NOT_AVAILABLE' })
    })
  })

  describe('installAndRestart', () => {
    it('returns UPDATE_NOT_AVAILABLE when update not downloaded', async () => {
      const result = await installAndRestart()

      expect(result).toEqual({
        success: false,
        error: 'No downloaded update ready to install',
        code: 'UPDATE_NOT_AVAILABLE'
      })
    })

    it('installs the downloaded package in Rust then relaunches', async () => {
      const update = createMockUpdate('2.1.0')
      vi.mocked(check).mockResolvedValue(update as never)
      await checkForUpdates()

      mockRustUpdater()
      await downloadUpdate()

      vi.mocked(relaunch).mockResolvedValue(undefined)
      const result = await installAndRestart()

      expect(update.install).not.toHaveBeenCalled()
      expect(invokeCalls('updater_install')).toEqual([['updater_install', { version: '2.1.0' }]])
      expect(relaunch).toHaveBeenCalledTimes(1)
      // install must run before relaunch.
      expect(invokeOrder('updater_install')).toBeLessThan(
        vi.mocked(relaunch).mock.invocationCallOrder[0]
      )
      expect(result).toEqual({ success: true, data: undefined })
    })

    it('returns INSTALL_FAILED when the Rust install fails', async () => {
      const update = createMockUpdate('2.1.2')
      vi.mocked(check).mockResolvedValue(update as never)
      await checkForUpdates()

      mockRustUpdater({
        install: () => ({ success: false, error: 'install failed', code: 'INSTALL_FAILED' })
      })
      await downloadUpdate()

      const result = await installAndRestart()

      expect(relaunch).not.toHaveBeenCalled()
      expect(result).toEqual({
        success: false,
        error: 'install failed',
        code: 'INSTALL_FAILED'
      })
    })

    it('returns INSTALL_FAILED when relaunch throws', async () => {
      const update = createMockUpdate('2.1.1')
      vi.mocked(check).mockResolvedValue(update as never)
      await checkForUpdates()

      mockRustUpdater()
      await downloadUpdate()

      vi.mocked(relaunch).mockRejectedValue(new Error('relaunch failed'))
      const result = await installAndRestart()

      expect(result).toEqual({
        success: false,
        error: 'relaunch failed',
        code: 'INSTALL_FAILED'
      })
    })

    it('preserves the downloaded update across a re-check of the same version', async () => {
      const update = createMockUpdate('2.1.0')
      vi.mocked(check).mockResolvedValue(update as never)
      await checkForUpdates()

      mockRustUpdater()
      await downloadUpdate()

      // A periodic re-check returns the SAME version; the downloaded state
      // must survive so install still works.
      vi.mocked(check).mockResolvedValue(update as never)
      await checkForUpdates()

      vi.mocked(relaunch).mockResolvedValue(undefined)
      const result = await installAndRestart()

      expect(invokeCalls('updater_install')).toHaveLength(1)
      expect(relaunch).toHaveBeenCalledTimes(1)
      expect(result).toEqual({ success: true, data: undefined })
    })
  })

  describe('state and helpers', () => {
    it('clearPendingUpdate resets pending and downloaded state', async () => {
      const update = createMockUpdate('2.2.0')
      vi.mocked(check).mockResolvedValue(update as never)
      await checkForUpdates()

      await clearPendingUpdate()
      const state = await getUpdaterState()

      expect(state.success).toBe(true)
      if (state.success) {
        expect(state.data.updateAvailable).toBe(false)
        expect(state.data.downloaded).toBe(false)
        expect(state.data.version).toBeNull()
      }
    })

    it('set/get autoUpdateEnabled round-trips value', async () => {
      await setAutoUpdateEnabled(false)
      const disabled = await getAutoUpdateEnabled()
      expect(disabled).toEqual({ success: true, data: false })

      await setAutoUpdateEnabled(true)
      const enabled = await getAutoUpdateEnabled()
      expect(enabled).toEqual({ success: true, data: true })
    })

    it('mapTauriUpdateToInfo maps body/date correctly', () => {
      const info = mapTauriUpdateToInfo(
        createMockUpdate('3.0.0', 'notes', '2026-03-01T12:00:00.000Z') as never
      )

      expect(info).toEqual({
        version: '3.0.0',
        releaseDate: '2026-03-01T12:00:00.000Z',
        releaseNotes: 'notes',
        componentPolicy: legacyUpdateComponentPolicy('3.0.0'),
        isSecurityUpdate: false
      })
    })

    it('normalizes the public Plugin Core extension from a signed update manifest', async () => {
      const legacy = legacyUpdateComponentPolicy('3.0.0')
      const { pluginCore: _unsupported, ...components } = legacy.components
      const pluginCore = {
        buildId: `termul-pluginCore-v1-sha256:${'b'.repeat(64)}`,
        action: 'defer-if-active'
      }
      const update = createMockUpdate('3.0.0')
      vi.mocked(check).mockResolvedValue(update as never)
      vi.mocked(invoke).mockResolvedValueOnce({
        success: true,
        data: {
          version: '3.0.0',
          termul: {
            componentPolicy: {
              ...legacy,
              metadataState: 'declared',
              components,
              pluginCore
            }
          }
        }
      })
      const info = await checkForUpdates()
      expect(info?.componentPolicy?.components.pluginCore).toEqual(pluginCore)
      expect(Object.keys(info?.componentPolicy?.components ?? {})).toHaveLength(5)
    })

    it('isUpdateAvailable returns boolean guard semantics', () => {
      expect(isUpdateAvailable(null)).toBe(false)
      expect(isUpdateAvailable(createMockUpdate('1.0.0') as never)).toBe(true)
    })

    it('registerUpdateEventHandlers initializes recovery metadata and returns cleanup', async () => {
      const cleanup = registerUpdateEventHandlers({
        onError: vi.fn()
      })

      await flushPromises()

      expect(getVersion).toHaveBeenCalledTimes(1)
      expect(setAppVersion).toHaveBeenCalledWith('0.2.3')
      expect(setCurrentVersion).toHaveBeenCalledWith('0.2.3')
      expect(typeof cleanup).toBe('function')
      expect(() => cleanup()).not.toThrow()
    })

    it('registerUpdateEventHandlers reports initialization errors via onError', async () => {
      const onError = vi.fn()
      vi.mocked(getVersion).mockRejectedValue(new Error('app unavailable'))

      registerUpdateEventHandlers({ onError })
      await flushPromises()

      expect(onError).toHaveBeenCalledWith(
        'Failed to initialize updater recovery metadata: app unavailable'
      )
    })
  })
})
