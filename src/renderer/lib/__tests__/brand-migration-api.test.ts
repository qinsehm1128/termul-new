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
import type {
  BrandMigrationReceipt,
  BrandMigrationRun,
  LegacyDataDetection
} from '../brand-migration-api'
import {
  browserBrandMigrationApi,
  createTauriBrandMigrationApi,
  hasFailedRoots
} from '../brand-migration-api'
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

const run: BrandMigrationRun = {
  runId: '2b6d6a05-3bdd-4dcb-8434-f3a8a1854457',
  startedAtUtc: '2026-09-05T03:21:00Z',
  roots: receipt.roots,
  notices: [{ id: 'M-03', status: 'notApplicable', detail: 'cache is rebuilt on demand' }]
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

  describe('lastRun', () => {
    it('unwraps the IpcResult success envelope, including the null case', async () => {
      mockInvoke.mockResolvedValue({ success: true, data: run })

      const result = await createTauriBrandMigrationApi().lastRun()

      expect(mockInvoke).toHaveBeenCalledWith('brand_migration_last_run', undefined)
      expect(result).toEqual(run)

      // A host that never merged answers `null` inside a *successful* envelope;
      // reading that as a failure would make the banner prompt forever again.
      mockInvoke.mockResolvedValue({ success: true, data: null })
      await expect(createTauriBrandMigrationApi().lastRun()).resolves.toBeNull()
    })

    it('degrades to null and warns when the host cannot read the journal', async () => {
      mockInvoke.mockResolvedValue({
        success: false,
        error: 'journal unreadable',
        code: 'BRAND_MIGRATION_FAILED'
      })

      await expect(createTauriBrandMigrationApi().lastRun()).resolves.toBeNull()
      expect(mockLogFrontendError).toHaveBeenCalledWith({
        level: 'warn',
        source: 'brand-migration.lastRun',
        message: 'brand_migration_last_run failed: journal unreadable (BRAND_MIGRATION_FAILED)'
      })
    })

    it('returns null without invoking outside the Tauri runtime', async () => {
      mockIsTauriContext.mockReturnValue(false)

      await expect(createTauriBrandMigrationApi().lastRun()).resolves.toBeNull()
      expect(mockInvoke).not.toHaveBeenCalled()
    })
  })

  /**
   * The predicate the banner keys its prompt on. Getting it backwards in either
   * direction is a real bug with no visible symptom: too eager and a user with
   * unmigrated data is never told, too shy and the banner nags forever.
   */
  describe('hasFailedRoots', () => {
    it('is false for no run and for a run whose rows all landed', () => {
      expect(hasFailedRoots(null)).toBe(false)
      expect(hasFailedRoots(run)).toBe(false)
      expect(hasFailedRoots({ ...run, roots: [] })).toBe(false)
    })

    it('is true as soon as one row failed, whatever the others did', () => {
      expect(
        hasFailedRoots({
          ...run,
          roots: [
            { kind: 'appDataDir', label: 'App data', status: 'migrated', reason: null },
            { kind: 'keychainService', label: 'Keychain', status: 'failed', reason: 'locked' }
          ]
        })
      ).toBe(true)
    })
  })

  describe('browser impl', () => {
    it('reports nothing to migrate and never touches IPC', async () => {
      await expect(browserBrandMigrationApi.detectLegacyData()).resolves.toBeNull()
      await expect(browserBrandMigrationApi.lastRun()).resolves.toBeNull()
      await expect(browserBrandMigrationApi.runMigration()).rejects.toThrow(
        'run_brand_migration is desktop-only'
      )
      expect(mockInvoke).not.toHaveBeenCalled()
    })
  })
})
