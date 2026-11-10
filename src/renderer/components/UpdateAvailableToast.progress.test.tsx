/**
 * The download progress toast must be updated in place while the download
 * runs. Dismissing it on every progress event made it flash in and out.
 */
import { renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('sonner', () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn()
  })
}))

const state = {
  updateAvailable: true,
  downloaded: false,
  isDownloading: true,
  isPreparingDownload: false,
  skippedVersion: null as string | null,
  version: '1.2.3',
  downloadProgress: 0
}

vi.mock('@/stores/updater-store', () => ({
  updaterStore: { getState: () => ({}) },
  useUpdaterState: () => ({
    updateAvailable: state.updateAvailable,
    downloaded: state.downloaded,
    isDownloading: state.isDownloading,
    isPreparingDownload: state.isPreparingDownload,
    skippedVersion: state.skippedVersion
  }),
  useUpdaterActions: () => ({}),
  useUpdateVersion: () => state.version,
  useUpdateDownloaded: () => state.downloaded,
  useIsDownloading: () => state.isDownloading,
  useDownloadProgress: () => state.downloadProgress
}))

vi.mock('@/lib/tauri-updater-api', () => ({ isAurUpdateMode: vi.fn(() => false) }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: vi.fn() }))
vi.mock('@/lib/router-navigate', () => ({ navigateToPath: vi.fn() }))
vi.mock('@/lib/tauri-safe-update', () => ({ hasActiveTerminalSessions: () => false }))

import { toast } from 'sonner'
import { useUpdateToast } from './UpdateAvailableToast'

describe('useUpdateToast download progress', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    state.isDownloading = true
    state.downloaded = false
    state.downloadProgress = 0
    state.version = '1.2.3'
  })

  it('updates the same progress toast without dismissing it between events', () => {
    const { rerender } = renderHook(() => useUpdateToast())
    for (const progress of [10, 35, 70, 95]) {
      state.downloadProgress = progress
      rerender()
    }

    const loading = vi.mocked(toast.loading)
    expect(loading).toHaveBeenCalledTimes(5)
    for (const call of loading.mock.calls) {
      expect(call[1]).toEqual(expect.objectContaining({ id: 'download-progress-1.2.3' }))
    }
    expect(toast.dismiss).not.toHaveBeenCalled()
  })

  it('dismisses the progress toast once the download ends', () => {
    const { rerender } = renderHook(() => useUpdateToast())
    state.isDownloading = false
    state.downloaded = true
    state.downloadProgress = 100
    rerender()

    expect(toast.dismiss).toHaveBeenCalledWith('download-progress-1.2.3')
    expect(toast.dismiss).toHaveBeenCalledTimes(1)
  })
})
