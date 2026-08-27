import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { fetchMock, openPaneMock } = vi.hoisted(() => ({
  fetchMock: vi.fn(),
  openPaneMock: vi.fn()
}))

vi.mock('@/lib/macos-permissions-api', () => ({
  fetchPermissionReport: (...args: unknown[]) => fetchMock(...args),
  openPrivacyPane: (...args: unknown[]) => openPaneMock(...args)
}))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn()
}))

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key })
}))

import type { PermissionProbe, PermissionReport } from '@/lib/macos-permissions-api'
import { MacosPermissionsSettings } from './MacosPermissionsSettings'

function probe(
  id: PermissionProbe['id'],
  state: PermissionProbe['state'],
  active: boolean,
  detail: string | null = null
): PermissionProbe {
  return { id, state, active, detail }
}

function report(overrides: Partial<PermissionReport> = {}): PermissionReport {
  return {
    supported: true,
    osVersion: '26.0',
    bundleId: 'com.termul-manager.app',
    signing: { kind: 'adhoc', teamId: null, grantsSurviveRebuild: false },
    probes: [
      probe('fullDiskAccess', 'denied', false, 'EACCES /Library/.../TCC.db'),
      probe('accessibility', 'denied', false),
      probe('localNetwork', 'notProbed', true),
      probe('desktopFolder', 'notProbed', true)
    ],
    ...overrides
  }
}

function rows(): string[] {
  return Array.from(document.querySelectorAll('[data-permission-row]')).map(
    (node) => (node as HTMLElement).dataset.permissionRow ?? ''
  )
}

describe('MacosPermissionsSettings', () => {
  beforeEach(() => {
    fetchMock.mockReset()
    openPaneMock.mockReset()
    openPaneMock.mockResolvedValue(undefined)
  })

  it('runs only the passive sweep on mount', async () => {
    // A probe with a side effect must never fire without a click, or opening
    // Settings would pop a system prompt out of nowhere.
    fetchMock.mockResolvedValue(report())
    render(<MacosPermissionsSettings />)

    await waitFor(() => expect(fetchMock).toHaveBeenCalled())
    expect(fetchMock).toHaveBeenCalledWith([])
  })

  it('orders rows by consequence rather than by backend order', async () => {
    fetchMock.mockResolvedValue(report())
    render(<MacosPermissionsSettings />)

    await waitFor(() => expect(rows().length).toBe(4))
    expect(rows()).toEqual(['localNetwork', 'fullDiskAccess', 'accessibility', 'desktopFolder'])
  })

  it('renders every id the backend can report', async () => {
    // The ordering drops what it cannot place, so an id missing from the
    // display order would vanish from the panel entirely. `UnplacedIds` in the
    // component turns that into a compile error; this pins the runtime half.
    const everyId: PermissionProbe['id'][] = [
      'fullDiskAccess',
      'accessibility',
      'screenRecording',
      'inputMonitoring',
      'localNetwork',
      'desktopFolder',
      'documentsFolder',
      'downloadsFolder'
    ]
    fetchMock.mockResolvedValue(
      report({ probes: everyId.map((id) => probe(id, 'unknown', false)) })
    )
    render(<MacosPermissionsSettings />)

    await waitFor(() => expect(rows()).toHaveLength(everyId.length))
    expect([...rows()].sort()).toEqual([...everyId].sort())
  })

  it('offers a check button only where probing has a side effect', async () => {
    fetchMock.mockResolvedValue(report())
    render(<MacosPermissionsSettings />)

    await waitFor(() => expect(rows().length).toBe(4))
    const checks = screen.getAllByRole('button', { name: 'macosPrivacy.check' })
    expect(checks).toHaveLength(2)
  })

  it('probes the row that was clicked', async () => {
    fetchMock.mockResolvedValue(report())
    render(<MacosPermissionsSettings />)
    await waitFor(() => expect(rows().length).toBe(4))

    const localNetworkRow = document.querySelector('[data-permission-row="localNetwork"]')
    const check = localNetworkRow?.querySelector('button')
    fireEvent.click(check as Element)

    await waitFor(() => expect(fetchMock).toHaveBeenLastCalledWith(['localNetwork']))
  })

  it('keeps an already-probed row probed when another row is checked', async () => {
    // Each check re-runs the whole report. Dropping the earlier consent would
    // reset that row to "not checked" and throw away the answer the user just
    // granted a prompt for.
    fetchMock.mockResolvedValue(report())
    render(<MacosPermissionsSettings />)
    await waitFor(() => expect(rows().length).toBe(4))

    const clickCheckIn = (id: string): void => {
      const row = document.querySelector(`[data-permission-row="${id}"]`)
      fireEvent.click(row?.querySelector('button') as Element)
    }

    clickCheckIn('localNetwork')
    await waitFor(() => expect(fetchMock).toHaveBeenLastCalledWith(['localNetwork']))

    clickCheckIn('desktopFolder')
    await waitFor(() =>
      expect(fetchMock).toHaveBeenLastCalledWith(['localNetwork', 'desktopFolder'])
    )
  })

  it('reflects the state each probe came back with', async () => {
    fetchMock.mockResolvedValue(
      report({
        probes: [probe('localNetwork', 'granted', true), probe('accessibility', 'denied', false)]
      })
    )
    render(<MacosPermissionsSettings />)

    await waitFor(() => expect(rows().length).toBe(2))
    expect(
      document
        .querySelector('[data-permission-row="localNetwork"]')
        ?.getAttribute('data-permission-state')
    ).toBe('granted')
    expect(
      document
        .querySelector('[data-permission-row="accessibility"]')
        ?.getAttribute('data-permission-state')
    ).toBe('denied')
  })

  it('warns when the code identity will not survive a rebuild', async () => {
    fetchMock.mockResolvedValue(report())
    render(<MacosPermissionsSettings />)

    await waitFor(() =>
      expect(screen.getByText('macosPrivacy.unstableIdentity')).toBeInTheDocument()
    )
  })

  it('drops the identity warning once the build is signed', async () => {
    fetchMock.mockResolvedValue(
      report({
        signing: { kind: 'developerId', teamId: 'ABCDE12345', grantsSurviveRebuild: true }
      })
    )
    render(<MacosPermissionsSettings />)

    await waitFor(() => expect(rows().length).toBe(4))
    expect(screen.queryByText('macosPrivacy.unstableIdentity')).not.toBeInTheDocument()
  })

  it('surfaces a failed probe instead of showing a silently empty list', async () => {
    fetchMock.mockRejectedValue(new Error('probe exploded'))
    render(<MacosPermissionsSettings />)

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('probe exploded'))
  })

  it('opens the settings pane for the row it belongs to', async () => {
    fetchMock.mockResolvedValue(report())
    render(<MacosPermissionsSettings />)
    await waitFor(() => expect(rows().length).toBe(4))

    const row = document.querySelector('[data-permission-row="accessibility"]')
    const buttons = row?.querySelectorAll('button') ?? []
    // Passive rows carry only the Open Settings button.
    fireEvent.click(buttons[buttons.length - 1] as Element)

    expect(openPaneMock).toHaveBeenCalledWith('accessibility')
  })

  it('re-runs every consented probe from the footer button', async () => {
    fetchMock.mockResolvedValue(report())
    render(<MacosPermissionsSettings />)
    await waitFor(() => expect(rows().length).toBe(4))

    const row = document.querySelector('[data-permission-row="localNetwork"]')
    fireEvent.click(row?.querySelector('button') as Element)
    await waitFor(() => expect(fetchMock).toHaveBeenLastCalledWith(['localNetwork']))

    fireEvent.click(screen.getByRole('button', { name: 'macosPrivacy.recheck' }))

    await waitFor(() => expect(fetchMock).toHaveBeenLastCalledWith(['localNetwork']))
    expect(fetchMock).toHaveBeenCalledTimes(3)
  })
})
