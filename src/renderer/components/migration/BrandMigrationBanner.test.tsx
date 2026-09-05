import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  BrandMigrationReceipt,
  BrandMigrationRootReceipt,
  BrandMigrationRun,
  LegacyDataDetection,
  LegacyDataSignal,
  SshKnownHostsStatus
} from '@/lib/brand-migration-api'
import { BrandMigrationBanner } from './BrandMigrationBanner'

const { mockDetectLegacyData, mockLastRun, mockRunMigration } = vi.hoisted(() => ({
  mockDetectLegacyData: vi.fn(),
  mockLastRun: vi.fn(),
  mockRunMigration: vi.fn()
}))

// `importOriginal` rather than a bare factory: the component also imports the
// pure `hasFailedRoots` from this module, and a hand-written stand-in for it
// would let the banner's suppression rule and the rule the app ships diverge
// without a single test noticing.
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
  return {
    kind,
    label: `label:${kind}`,
    path: `/legacy/${kind}`,
    present: true,
    ...overrides
  }
}

function detection(overrides: Partial<LegacyDataDetection> = {}): LegacyDataDetection {
  return {
    hasLegacyData: true,
    signals: [signal('appDataDir'), signal('documentsWorkspace')],
    sshKnownHosts: { state: 'notApplicable' },
    tccNotice: null,
    ...overrides
  }
}

const receipt: BrandMigrationReceipt = {
  roots: [
    { kind: 'appDataDir', label: 'label:appDataDir', status: 'migrated', reason: null },
    {
      kind: 'documentsWorkspace',
      label: 'label:documentsWorkspace',
      status: 'skipped',
      reason: null
    }
  ]
}

const sshFailed: SshKnownHostsStatus = { state: 'failed', reason: 'permission denied' }

/** A recorded pass whose rows carry `statuses`, in order. */
function recordedRun(...statuses: BrandMigrationRootReceipt['status'][]): BrandMigrationRun {
  return {
    runId: '2b6d6a05-3bdd-4dcb-8434-f3a8a1854457',
    startedAtUtc: '2026-09-05T03:21:00Z',
    roots: statuses.map((status, index) => ({
      kind: index === 0 ? 'appDataDir' : 'documentsWorkspace',
      label: `label:row${index}`,
      status,
      reason: null
    })),
    notices: []
  }
}

async function renderBanner(
  value: LegacyDataDetection | null,
  lastRun: BrandMigrationRun | null = null
): Promise<void> {
  mockDetectLegacyData.mockResolvedValue(value)
  mockLastRun.mockResolvedValue(lastRun)
  render(<BrandMigrationBanner />)
  if (value === null) return
  await waitFor(() => {
    expect(mockDetectLegacyData).toHaveBeenCalled()
  })
}

describe('BrandMigrationBanner', () => {
  beforeEach(() => {
    mockDetectLegacyData.mockReset()
    mockLastRun.mockReset()
    mockLastRun.mockResolvedValue(null)
    mockRunMigration.mockReset()
    mockRunMigration.mockResolvedValue(receipt)
  })

  afterEach(() => {
    cleanup()
  })

  it('renders nothing when detection reports no legacy data (browser surface)', async () => {
    await renderBanner(null)

    await waitFor(() => {
      expect(mockDetectLegacyData).toHaveBeenCalledTimes(1)
    })
    expect(screen.queryByTestId('brand-migration-banner')).toBeNull()

    // Same on desktop when the probe ran and found nothing worth merging.
    cleanup()
    await renderBanner(detection({ hasLegacyData: false, signals: [] }))
    expect(screen.queryByTestId('brand-migration-banner')).toBeNull()
  })

  it('lists one entry per present signal, in order, and states that nothing is deleted', async () => {
    await renderBanner(
      detection({
        signals: [
          signal('appDataDir'),
          signal('localStorage', { path: null }),
          signal('repoWorkspaceDir', { present: false }),
          signal('documentsWorkspace')
        ]
      })
    )

    const items = await screen.findAllByTestId('brand-migration-root')
    expect(items.map((item) => item.getAttribute('data-kind'))).toEqual([
      'appDataDir',
      'localStorage',
      'documentsWorkspace'
    ])
    expect(items.map((item) => item.textContent)).toEqual([
      'label:appDataDir /legacy/appDataDirWill be merged',
      'label:localStorage No file path (stored by the system)Will be merged',
      'label:documentsWorkspace /legacy/documentsWorkspaceWill be merged'
    ])
    expect(screen.getByTestId('brand-migration-copy-only')).toHaveTextContent(
      'Merging only copies. Your old data is never deleted.'
    )
  })

  it('renders the TCC notice on macOS and omits it everywhere else', async () => {
    await renderBanner(detection({ tccNotice: 'Screen recording must be granted again.' }))
    expect(await screen.findByTestId('brand-migration-tcc')).toHaveTextContent(
      'Screen recording must be granted again.'
    )
    expect(screen.getByTestId('brand-migration-tcc')).toHaveTextContent('macOS privacy permissions')

    cleanup()

    await renderBanner(detection({ tccNotice: null }))
    await screen.findByTestId('brand-migration-banner')
    expect(screen.queryByTestId('brand-migration-tcc')).toBeNull()
  })

  it('invokes the migration command exactly once when the primary action is clicked', async () => {
    await renderBanner(detection())

    const start = await screen.findByRole('button', { name: 'Start merge' })
    fireEvent.click(start)

    await waitFor(() => {
      expect(screen.getByTestId('brand-migration-done')).toBeInTheDocument()
    })
    expect(mockRunMigration).toHaveBeenCalledTimes(1)

    // The action is spent: a second click cannot fire a second migration.
    fireEvent.click(screen.getByRole('button', { name: 'Start merge' }))
    expect(mockRunMigration).toHaveBeenCalledTimes(1)

    const statuses = screen
      .getAllByTestId('brand-migration-root')
      .map((item) => item.getAttribute('data-status'))
    expect(statuses).toEqual(['migrated', 'skipped'])
  })

  it('keeps the banner away for the rest of the session once "Later" is clicked', async () => {
    await renderBanner(detection())

    fireEvent.click(await screen.findByRole('button', { name: 'Later' }))
    expect(screen.queryByTestId('brand-migration-banner')).toBeNull()

    // A fresh mount in the same session must not prompt again.
    cleanup()
    await renderBanner(detection())
    await waitFor(() => {
      expect(mockDetectLegacyData).toHaveBeenCalledTimes(2)
    })
    expect(screen.queryByTestId('brand-migration-banner')).toBeNull()
    expect(mockRunMigration).not.toHaveBeenCalled()
  })

  it('raises a named SSH known-hosts warning that "Later" cannot swallow', async () => {
    await renderBanner(detection({ sshKnownHosts: sshFailed }))

    const warning = await screen.findByTestId('brand-migration-ssh-warning')
    expect(warning).toHaveAccessibleName('SSH known-hosts migration failed')
    expect(warning).toHaveAttribute('role', 'alert')
    expect(warning).toHaveTextContent(
      'Host-key checking is now fail-closed: hosts you had already trusted may refuse to connect until you confirm their keys again.'
    )
    expect(warning).toHaveTextContent('Reason: permission denied')

    fireEvent.click(screen.getByRole('button', { name: 'Later' }))

    // "Later" dismisses the merge prompt only. The security consequence stays.
    expect(screen.queryByRole('button', { name: 'Start merge' })).toBeNull()
    expect(screen.getByTestId('brand-migration-ssh-warning')).toHaveAccessibleName(
      'SSH known-hosts migration failed'
    )

    // And it survives a remount in the same session, too.
    cleanup()
    await renderBanner(detection({ sshKnownHosts: sshFailed }))
    expect(await screen.findByTestId('brand-migration-ssh-warning')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Start merge' })).toBeNull()
  })

  it('shows a successful startup SSH migration among the already-merged roots', async () => {
    await renderBanner(
      detection({
        signals: [signal('appDataDir'), signal('sshKnownHosts')],
        sshKnownHosts: { state: 'migrated' }
      })
    )

    const items = await screen.findAllByTestId('brand-migration-root')
    expect(
      items.map((item) => [item.getAttribute('data-kind'), item.getAttribute('data-status')])
    ).toEqual([
      ['appDataDir', 'pending'],
      ['sshKnownHosts', 'migrated']
    ])
    expect(items[1]).toHaveTextContent('Already merged')
    expect(screen.queryByTestId('brand-migration-ssh-warning')).toBeNull()
  })

  it('renders the receipt sshKnownHosts row, so a host-side re-run cannot hide behind the startup status', async () => {
    const sshSignals = [signal('appDataDir'), signal('sshKnownHosts')]

    // Faithful case: T-MIG-RUN carries the M-15 line verbatim from the startup
    // pass, so the receipt row reads exactly what detection reported.
    mockRunMigration.mockResolvedValue({
      roots: [
        { kind: 'appDataDir', label: 'label:appDataDir', status: 'migrated', reason: null },
        {
          kind: 'sshKnownHosts',
          label: 'label:sshKnownHosts',
          status: 'failed',
          reason: 'permission denied'
        }
      ]
    })
    await renderBanner(detection({ signals: sshSignals, sshKnownHosts: sshFailed }))
    fireEvent.click(await screen.findByRole('button', { name: 'Start merge' }))
    await waitFor(() => {
      expect(screen.getByTestId('brand-migration-done')).toBeInTheDocument()
    })
    expect(
      screen
        .getAllByTestId('brand-migration-root')
        .map((item) => [item.getAttribute('data-kind'), item.getAttribute('data-status')])
    ).toEqual([
      ['appDataDir', 'migrated'],
      ['sshKnownHosts', 'failed']
    ])

    // Regression case: the host re-ran M-15 and overwrote the status. The row
    // must show what the receipt said — the contradiction against the startup
    // warning below it is the whole point. Masking it with the startup value
    // would make the forbidden re-run invisible.
    cleanup()
    mockRunMigration.mockResolvedValue({
      roots: [
        {
          kind: 'sshKnownHosts',
          label: 'label:sshKnownHosts',
          status: 'migrated',
          reason: null
        }
      ]
    })
    await renderBanner(detection({ signals: sshSignals, sshKnownHosts: sshFailed }))
    fireEvent.click(await screen.findByRole('button', { name: 'Start merge' }))
    await waitFor(() => {
      expect(screen.getByTestId('brand-migration-done')).toBeInTheDocument()
    })
    const sshRow = screen
      .getAllByTestId('brand-migration-root')
      .find((item) => item.getAttribute('data-kind') === 'sshKnownHosts')
    expect(sshRow).toHaveAttribute('data-status', 'migrated')
    expect(sshRow).toHaveTextContent('Already merged')
    // …while the startup failure is still reported, so the two disagree loudly.
    expect(screen.getByTestId('brand-migration-ssh-warning')).toHaveAccessibleName(
      'SSH known-hosts migration failed'
    )
  })

  /**
   * The defect this whole predicate exists for. The merge copies and never
   * deletes, so every legacy root is still on disk the moment it finishes and
   * `hasLegacyData` never goes false again. Keying the prompt on that alone
   * meant the banner came back at every single app start, forever, to a user
   * with nothing left to do — and "Later" is session-scoped, so nothing
   * suppressed it across restarts either.
   */
  it('stays away once a clean pass is recorded, even though the legacy data is still there', async () => {
    await renderBanner(detection(), recordedRun('migrated', 'skipped'))

    await waitFor(() => {
      expect(mockLastRun).toHaveBeenCalled()
    })
    expect(screen.queryByTestId('brand-migration-banner')).toBeNull()
    // Not because the probe went quiet — it still reports every root.
    expect(await mockDetectLegacyData.mock.results[0]?.value).toMatchObject({
      hasLegacyData: true
    })
  })

  it('keeps prompting when the recorded pass left a root failed, and offers a permanent way out', async () => {
    await renderBanner(detection(), recordedRun('migrated', 'failed'))

    // Still owed work, so the prompt is due.
    expect(await screen.findByTestId('brand-migration-banner')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Start merge' })).toBeInTheDocument()

    fireEvent.click(screen.getByTestId('brand-migration-silence'))
    expect(screen.queryByTestId('brand-migration-banner')).toBeNull()

    // Unlike "Later", this one outlives the run.
    cleanup()
    await renderBanner(detection(), recordedRun('migrated', 'failed'))
    await waitFor(() => {
      expect(mockLastRun).toHaveBeenCalledTimes(2)
    })
    expect(screen.queryByTestId('brand-migration-banner')).toBeNull()
  })

  it('withholds the permanent dismissal until a pass has actually failed', async () => {
    // Nothing recorded yet: "Later" is the right escape hatch, and a permanent
    // one here would let a user bury work they have never even attempted.
    await renderBanner(detection(), null)
    await screen.findByTestId('brand-migration-banner')
    expect(screen.queryByTestId('brand-migration-silence')).toBeNull()
    expect(screen.getByRole('button', { name: 'Later' })).toBeInTheDocument()
  })

  it('points at the settings panel, so "Later" is not read as the last chance', async () => {
    await renderBanner(detection())

    expect(await screen.findByTestId('brand-migration-settings-hint')).toHaveTextContent(
      'You can run this any time from Settings → Data migration.'
    )
  })

  it('surfaces a failed migration instead of reporting success', async () => {
    mockRunMigration.mockRejectedValue(new Error('disk full'))
    await renderBanner(detection())

    fireEvent.click(await screen.findByRole('button', { name: 'Start merge' }))

    const failure = await screen.findByTestId('brand-migration-error')
    expect(failure).toHaveTextContent('Merge failed — Reason: disk full')
    expect(screen.queryByTestId('brand-migration-done')).toBeNull()
  })
})
