import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  BrandMigrationReceipt,
  LegacyDataDetection,
  LegacyDataSignal,
  SshKnownHostsStatus
} from '@/lib/brand-migration-api'
import { BrandMigrationBanner } from './BrandMigrationBanner'

const { mockDetectLegacyData, mockRunMigration } = vi.hoisted(() => ({
  mockDetectLegacyData: vi.fn(),
  mockRunMigration: vi.fn()
}))

vi.mock('@/lib/brand-migration-api', () => ({
  brandMigrationApi: {
    detectLegacyData: mockDetectLegacyData,
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

async function renderBanner(value: LegacyDataDetection | null): Promise<void> {
  mockDetectLegacyData.mockResolvedValue(value)
  render(<BrandMigrationBanner />)
  if (value === null) return
  await waitFor(() => {
    expect(mockDetectLegacyData).toHaveBeenCalled()
  })
}

describe('BrandMigrationBanner', () => {
  beforeEach(() => {
    mockDetectLegacyData.mockReset()
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

  it('surfaces a failed migration instead of reporting success', async () => {
    mockRunMigration.mockRejectedValue(new Error('disk full'))
    await renderBanner(detection())

    fireEvent.click(await screen.findByRole('button', { name: 'Start merge' }))

    const failure = await screen.findByTestId('brand-migration-error')
    expect(failure).toHaveTextContent('Merge failed — Reason: disk full')
    expect(screen.queryByTestId('brand-migration-done')).toBeNull()
  })
})
