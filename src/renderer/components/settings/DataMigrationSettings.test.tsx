import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  BrandMigrationRun,
  LegacyDataDetection,
  LegacyDataSignal
} from '@/lib/brand-migration-api'
import { DataMigrationSettings } from './DataMigrationSettings'

const { mockDetectLegacyData, mockLastRun, mockRunMigration } = vi.hoisted(() => ({
  mockDetectLegacyData: vi.fn(),
  mockLastRun: vi.fn(),
  mockRunMigration: vi.fn()
}))

vi.mock('@/lib/brand-migration-api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/brand-migration-api')>()),
  brandMigrationApi: {
    detectLegacyData: mockDetectLegacyData,
    lastRun: mockLastRun,
    runMigration: mockRunMigration
  }
}))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn()
}))

function signal(
  kind: LegacyDataSignal['kind'],
  overrides: Partial<LegacyDataSignal> = {}
): LegacyDataSignal {
  return { kind, label: `label:${kind}`, path: `/legacy/${kind}`, present: true, ...overrides }
}

function detection(overrides: Partial<LegacyDataDetection> = {}): LegacyDataDetection {
  return {
    hasLegacyData: true,
    signals: [signal('appDataDir'), signal('keychainService', { path: null })],
    sshKnownHosts: { state: 'notApplicable' },
    tccNotice: null,
    ...overrides
  }
}

const cleanRun: BrandMigrationRun = {
  runId: '2b6d6a05-3bdd-4dcb-8434-f3a8a1854457',
  startedAtUtc: '2026-09-05T03:21:00Z',
  roots: [
    { kind: 'appDataDir', label: 'App data', status: 'migrated', reason: '12 file(s) carried' },
    { kind: 'keychainService', label: 'Keychain', status: 'skipped', reason: null }
  ],
  notices: [{ id: 'M-03', status: 'notApplicable', detail: 'ACP registry cache: not migrated.' }]
}

async function renderPanel(
  probe: LegacyDataDetection | null = detection(),
  lastRun: BrandMigrationRun | null = null
): Promise<void> {
  mockDetectLegacyData.mockResolvedValue(probe)
  mockLastRun.mockResolvedValue(lastRun)
  render(<DataMigrationSettings />)
  await waitFor(() => {
    expect(mockLastRun).toHaveBeenCalled()
  })
}

function statuses(): [string | null, string | null][] {
  return Array.from(document.querySelectorAll('[data-migration-row]')).map((row) => [
    row.getAttribute('data-migration-row'),
    row.getAttribute('data-migration-status')
  ])
}

describe('DataMigrationSettings', () => {
  beforeEach(() => {
    mockDetectLegacyData.mockReset()
    mockLastRun.mockReset()
    mockRunMigration.mockReset()
    mockRunMigration.mockResolvedValue({ roots: [] })
  })

  afterEach(() => {
    cleanup()
  })

  /**
   * The banner filters to what is actionable because it is interrupting. This
   * panel was opened on purpose, and "we looked and there was nothing" is a
   * useful answer here — a row that silently vanishes is not.
   */
  it('lists every root, not just the ones with data left behind', async () => {
    await renderPanel()

    expect(statuses()).toEqual([
      ['appDataDir', 'pending'],
      ['keychainService', 'pending'],
      ['standaloneStateRoot', 'notApplicable'],
      ['localStorage', 'notApplicable'],
      ['documentsWorkspace', 'notApplicable'],
      ['repoWorkspaceDir', 'notApplicable'],
      ['sshKnownHosts', 'notApplicable']
    ])
    expect(screen.getByTestId('data-migration-summary')).toHaveTextContent(
      'This merge has not run on this computer yet.'
    )
  })

  /**
   * The journal outranks the probe wherever it has an entry: it records what the
   * app actually did, while the probe only reports what is still lying around —
   * which, since the merge never deletes, is everything.
   */
  it('lets the recorded run override the probe, and reports when it ran', async () => {
    await renderPanel(detection(), cleanRun)

    expect(statuses().slice(0, 2)).toEqual([
      ['appDataDir', 'migrated'],
      ['keychainService', 'skipped']
    ])
    expect(screen.getByTestId('data-migration-summary')).toHaveTextContent('Last merge:')
    // The per-root reason from the journal is what explains a bare status.
    expect(document.querySelector('[data-migration-row="appDataDir"]')).toHaveTextContent(
      '12 file(s) carried'
    )
    // Plan roots with no row of their own still reach the user here.
    expect(screen.getByTestId('data-migration-notices')).toHaveTextContent(
      'ACP registry cache: not migrated.'
    )
  })

  /**
   * The merge skips destinations that already exist, so a repeat pass is free.
   * Disabling the button after one run is what forced a user with a failed root
   * to restart the app to try again.
   */
  it('keeps the action available after a run and re-reads the journal afterwards', async () => {
    await renderPanel(detection(), cleanRun)

    const button = screen.getByTestId('data-migration-start')
    expect(button).toBeEnabled()
    expect(button).toHaveTextContent('Run merge again')

    mockLastRun.mockResolvedValue({
      ...cleanRun,
      roots: [{ kind: 'appDataDir', label: 'App data', status: 'failed', reason: 'disk full' }]
    })
    fireEvent.click(button)

    await waitFor(() => {
      expect(mockRunMigration).toHaveBeenCalledTimes(1)
    })
    // Re-read, not the receipt in hand: the journal is what every other surface
    // reads, so a disagreement would be invisible here and visible everywhere else.
    await waitFor(() => {
      expect(mockLastRun).toHaveBeenCalledTimes(2)
    })
    await waitFor(() => {
      expect(document.querySelector('[data-migration-row="appDataDir"]')).toHaveAttribute(
        'data-migration-status',
        'failed'
      )
    })
    expect(screen.getByTestId('data-migration-start')).toBeEnabled()
  })

  it('surfaces a refused run instead of leaving the panel looking successful', async () => {
    mockRunMigration.mockRejectedValue(new Error('another process is already migrating'))
    await renderPanel()

    fireEvent.click(screen.getByTestId('data-migration-start'))

    expect(await screen.findByTestId('data-migration-error')).toHaveTextContent(
      'another process is already migrating'
    )
    expect(screen.getByTestId('data-migration-start')).toBeEnabled()
  })

  it('reports the startup SSH outcome and its reason without calling it pending work', async () => {
    await renderPanel(
      detection({
        signals: [signal('sshKnownHosts')],
        sshKnownHosts: { state: 'failed', reason: 'permission denied' }
      })
    )

    const row = document.querySelector('[data-migration-row="sshKnownHosts"]')
    expect(row).toHaveAttribute('data-migration-status', 'failed')
    expect(row).toHaveTextContent('permission denied')
  })
})
