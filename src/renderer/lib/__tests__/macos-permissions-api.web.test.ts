import { beforeEach, describe, expect, it, vi } from 'vitest'

const { invokeMock, isTauriRef } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  isTauriRef: { current: true }
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args)
}))

vi.mock('../tauri-runtime', () => ({
  isTauriContext: () => isTauriRef.current
}))

import {
  fetchPermissionReport,
  openPrivacyPane,
  type PermissionReport
} from '../macos-permissions-api'

const REPORT: PermissionReport = {
  supported: true,
  osVersion: '26.0',
  bundleId: 'com.termul-manager.app',
  signing: { kind: 'adhoc', teamId: null, grantsSurviveRebuild: false },
  probes: []
}

describe('macos-permissions-api', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    isTauriRef.current = true
  })

  it('passes the consented probe ids to the backend', () => {
    invokeMock.mockResolvedValue(REPORT)

    void fetchPermissionReport(['localNetwork', 'desktopFolder'])

    expect(invokeMock).toHaveBeenCalledWith('macos_permissions_report_command', {
      active: ['localNetwork', 'desktopFolder']
    })
  })

  it('asks for no side-effecting probe by default', () => {
    // The default call runs on mount; sending an id here would make the panel
    // pop a system prompt nobody clicked for.
    invokeMock.mockResolvedValue(REPORT)

    void fetchPermissionReport()

    expect(invokeMock).toHaveBeenCalledWith('macos_permissions_report_command', { active: [] })
  })

  it('reports unsupported on web instead of invoking', async () => {
    isTauriRef.current = false

    const report = await fetchPermissionReport(['localNetwork'])

    expect(report.supported).toBe(false)
    expect(report.probes).toEqual([])
    expect(invokeMock).not.toHaveBeenCalled()
  })

  it('sends a pane id rather than a URL', async () => {
    invokeMock.mockResolvedValue(undefined)

    await openPrivacyPane('localNetwork')

    expect(invokeMock).toHaveBeenCalledWith('macos_open_privacy_pane_command', {
      id: 'localNetwork'
    })
  })

  it('does not try to open a settings pane on web', async () => {
    isTauriRef.current = false

    await openPrivacyPane('localNetwork')

    expect(invokeMock).not.toHaveBeenCalled()
  })
})
