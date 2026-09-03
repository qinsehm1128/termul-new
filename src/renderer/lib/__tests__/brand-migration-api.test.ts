/**
 * Wire-shape tests for `brand-migration-api.ts`.
 *
 * The banner's own tests mock this facade wholesale, so without this file the
 * `IpcResult<T>` envelope would be entirely unverified — and an envelope
 * mismatch fails SILENTLY (the banner reads `hasLegacyData` off a wrapper,
 * finds `undefined`, and renders nothing). These pin the envelope on both
 * commands so a Rust-side shape change is a red test, not a missing feature.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// Mock @tauri-apps/api/core BEFORE importing the module under test.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn()
}))

vi.mock('../log-api', () => ({
  logFrontendError: vi.fn()
}))

vi.mock('../tauri-runtime', () => ({
  isTauriContext: vi.fn(() => true)
}))

import { invoke } from '@tauri-apps/api/core'
import type { BrandMigrationReceipt, LegacyDataDetection } from '../brand-migration-api'
import { browserBrandMigrationApi, createTauriBrandMigrationApi } from '../brand-migration-api'
import { logFrontendError } from '../log-api'
import { isTauriContext } from '../tauri-runtime'

const mockInvoke = invoke as ReturnType<typeof vi.fn>
const mockIsTauriContext = isTauriContext as ReturnType<typeof vi.fn>
const mockLogFrontendError = logFrontendError as ReturnType<typeof vi.fn>

const detection: LegacyDataDetection = {
  hasLegacyData: true,
  signals: [{ kind: 'appDataDir', label: 'App data', path: '/legacy/app', present: true }],
  sshKnownHosts: { state: 'migrated' },
  tccNotice: null
}

const receipt: BrandMigrationReceipt = {
  roots: [{ kind: 'appDataDir', label: 'App data', status: 'migrated', reason: null }]
}

describe('brandMigrationApi (Tauri IPC)', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockIsTauriContext.mockReturnValue(true)
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  describe('detectLegacyData', () => {
    it('unwraps the IpcResult success envelope', async () => {
      mockInvoke.mockResolvedValue({ success: true, data: detection })

      const result = await createTauriBrandMigrationApi().detectLegacyData()

      expect(mockInvoke).toHaveBeenCalledWith('detect_legacy_brand_data', undefined)
      expect(result).toEqual(detection)
    })

    it('degrades to null and warns when the host reports a failed probe', async () => {
      mockInvoke.mockResolvedValue({
        success: false,
        error: 'keychain locked',
        code: 'BRAND_DETECT_FAILED'
      })

      const result = await createTauriBrandMigrationApi().detectLegacyData()

      expect(result).toBeNull()
      expect(mockLogFrontendError).toHaveBeenCalledWith({
        level: 'warn',
        source: 'brand-migration.detect',
        message: 'detect_legacy_brand_data failed: keychain locked (BRAND_DETECT_FAILED)'
      })
    })

    it('degrades to null when invoke itself throws', async () => {
      mockInvoke.mockRejectedValue(new Error('IPC invoke failed'))

      const result = await createTauriBrandMigrationApi().detectLegacyData()

      expect(result).toBeNull()
      expect(mockLogFrontendError).toHaveBeenCalledWith(
        expect.objectContaining({
          level: 'warn',
          message: expect.stringContaining('IPC invoke failed') as unknown as string
        })
      )
    })

    it('returns null without invoking outside the Tauri runtime', async () => {
      mockIsTauriContext.mockReturnValue(false)

      const result = await createTauriBrandMigrationApi().detectLegacyData()

      expect(result).toBeNull()
      expect(mockInvoke).not.toHaveBeenCalled()
    })
  })

  describe('runMigration', () => {
    it('unwraps the IpcResult success envelope', async () => {
      mockInvoke.mockResolvedValue({ success: true, data: receipt })

      const result = await createTauriBrandMigrationApi().runMigration()

      expect(mockInvoke).toHaveBeenCalledWith('run_brand_migration', undefined)
      expect(result).toEqual(receipt)
    })

    it('throws the host error message when the run fails', async () => {
      mockInvoke.mockResolvedValue({
        success: false,
        error: 'disk full',
        code: 'BRAND_MIGRATION_FAILED'
      })

      await expect(createTauriBrandMigrationApi().runMigration()).rejects.toThrow('disk full')
    })

    it('throws when invoke itself throws', async () => {
      mockInvoke.mockRejectedValue(new Error('IPC invoke failed'))

      await expect(createTauriBrandMigrationApi().runMigration()).rejects.toThrow(
        'IPC invoke failed'
      )
    })

    it('refuses outside the Tauri runtime instead of reporting an empty success', async () => {
      mockIsTauriContext.mockReturnValue(false)

      await expect(createTauriBrandMigrationApi().runMigration()).rejects.toThrow(
        'run_brand_migration requires the Tauri runtime'
      )
      expect(mockInvoke).not.toHaveBeenCalled()
    })
  })

  describe('browser impl', () => {
    it('reports nothing to migrate and never touches IPC', async () => {
      await expect(browserBrandMigrationApi.detectLegacyData()).resolves.toBeNull()
      await expect(browserBrandMigrationApi.runMigration()).rejects.toThrow(
        'run_brand_migration is desktop-only'
      )
      expect(mockInvoke).not.toHaveBeenCalled()
    })
  })
})
