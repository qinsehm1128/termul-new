import {
  emptyMcpHostAccessPolicy,
  emptyMcpSshGrant,
  MCP_SSH_OPERATIONS,
  type McpHostAccessView
} from '@shared/types/mcp-host.types'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import en from '../../locales/en/mcp.json'
import { McpHostAccessPanel } from './McpHostAccessPanel'

const { get, put, call } = vi.hoisted(() => ({ get: vi.fn(), put: vi.fn(), call: vi.fn() }))
vi.mock('@/lib/mcp-host-api', () => ({
  getMcpHostAccess: get,
  putMcpHostAccess: put,
  callMcpHost: call
}))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: translate }) }))
function translate(key: string): string {
  return (
    (key
      .split('.')
      .reduce<unknown>(
        (value, part) =>
          value && typeof value === 'object' ? (value as Record<string, unknown>)[part] : undefined,
        en
      ) as string) ?? key
  )
}
function fixture(): McpHostAccessView {
  return {
    policy: emptyMcpHostAccessPolicy(),
    profiles: [
      {
        profileId: 'p',
        name: 'Production',
        host: 'host',
        port: 22,
        username: 'operator',
        authMethod: 'key',
        tags: [],
        fingerprint: 'fp-current'
      }
    ],
    projects: [
      { id: 'a', name: 'Alpha' },
      { id: 'b', name: 'Beta' }
    ],
    hostMode: 'LOCAL_OPERATOR'
  }
}
let view: McpHostAccessView
beforeEach(() => {
  vi.clearAllMocks()
  view = fixture()
  get.mockImplementation(async () => ({ success: true, data: structuredClone(view) }))
  put.mockImplementation(async (_revision, policy) => ({
    success: true,
    data: { ...policy, revision: 1 }
  }))
  call.mockResolvedValue({ success: true, data: {} })
})
afterEach(cleanup)
const check = (name: string) => screen.getByRole('checkbox', { name })
const button = (key: string) => screen.getByRole('button', { name: translate(`hostAccess.${key}`) })
async function ready(): Promise<void> {
  await screen.findByText('Production')
}

describe('host access management', () => {
  it('shows deny defaults, global principal warning, and no secret inputs', async () => {
    const { container } = render(<McpHostAccessPanel />)
    await ready()
    expect(check(en.hostAccess.enabled)).not.toBeChecked()
    expect(check(en.hostAccess.workspaceEnabled)).not.toBeChecked()
    expect(check(en.hostAccess.allProjects)).not.toBeChecked()
    expect(check(en.hostAccess.grantProfile)).not.toBeChecked()
    expect(screen.getByText(en.hostAccess.globalWarning)).toBeInTheDocument()
    expect(screen.getByText(en.hostAccess.executeWarning)).toBeInTheDocument()
    expect(screen.getByText(en.hostAccess.localWarning)).toBeInTheDocument()
    expect(container.querySelector('input[type=password]')).toBeNull()
    expect(call).not.toHaveBeenCalled()
  })
  it('saves selected project IDs, individual SSH grants, roots, POSIX acknowledgement and fingerprints', async () => {
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(check(en.hostAccess.enabled))
    fireEvent.click(check(en.hostAccess.workspaceEnabled))
    fireEvent.click(check('Alpha'))
    fireEvent.click(check(en.hostAccess.includePaths))
    fireEvent.click(check(en.hostAccess.grantProfile))
    for (const op of MCP_SSH_OPERATIONS)
      expect(
        check(
          `${en.hostAccess.operations[op]}${['write_file', 'upload', 'download', 'job_cancel'].includes(op) ? ` — ${en.hostAccess.highRisk}` : ''}`
        )
      ).not.toBeChecked()
    fireEvent.click(check(en.hostAccess.operations.execute))
    fireEvent.click(check(`${en.hostAccess.operations.upload} — ${en.hostAccess.highRisk}`))
    fireEvent.click(check(en.hostAccess.posixJobs))
    fireEvent.change(screen.getByRole('textbox', { name: en.hostAccess.remoteRoots }), {
      target: { value: '/srv\n/opt' }
    })
    fireEvent.change(screen.getByRole('textbox', { name: en.hostAccess.localRoots }), {
      target: { value: '/host-data' }
    })
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0]).toEqual([
      0,
      {
        schemaVersion: 1,
        revision: 0,
        enabled: true,
        workspace: { enabled: true, projectIds: ['a'], includePaths: true },
        ssh: {
          profiles: {
            p: {
              operations: ['execute', 'upload'],
              remoteRoots: ['/srv', '/opt'],
              localRoots: ['/host-data'],
              posixJobs: true
            }
          }
        },
        profileFingerprints: { p: 'fp-current' }
      }
    ])
    expect(call).not.toHaveBeenCalled()
  })
  it('uses null only for explicit all-project selection and removes revoked grants/fingerprints', async () => {
    view.policy.ssh.profiles.p = emptyMcpSshGrant()
    view.policy.profileFingerprints.p = 'fp-current'
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(check(en.hostAccess.allProjects))
    expect(screen.queryByRole('checkbox', { name: 'Alpha' })).toBeNull()
    fireEvent.click(check(en.hostAccess.grantProfile))
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1]).toMatchObject({
      workspace: { projectIds: null },
      ssh: { profiles: {} },
      profileFingerprints: {}
    })
  })
  it.each([
    'PUBLIC_TUNNEL',
    'public-tunnel'
  ])('locks management and active calls in %s', async (hostMode) => {
    view.hostMode = hostMode
    view.policy.enabled = true
    view.policy.ssh.profiles.p = {
      ...emptyMcpSshGrant(),
      operations: ['test_connection', 'job_list', 'job_cancel']
    }
    render(<McpHostAccessPanel />)
    await ready()
    expect(check(en.hostAccess.enabled)).toBeDisabled()
    expect(button('save')).toBeDisabled()
    expect(button('test')).toBeDisabled()
    expect(button('refreshJobs')).toBeDisabled()
    expect(screen.getByText(en.hostAccess.readOnly)).toBeInTheDocument()
    expect(call).not.toHaveBeenCalled()
  })
  it('keeps a conflicting draft without overwriting and reloads explicitly', async () => {
    put.mockResolvedValue({ success: false, code: 'REVISION_CONFLICT', error: 'private details' })
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(check(en.hostAccess.enabled))
    fireEvent.click(button('save'))
    await screen.findByText(en.hostAccess.conflict)
    expect(check(en.hostAccess.enabled)).toBeChecked()
    expect(screen.queryByText('private details')).toBeNull()
    fireEvent.click(button('reload'))
    await waitFor(() => expect(check(en.hostAccess.enabled)).not.toBeChecked())
  })
  it.each([
    'FORBIDDEN',
    'UNAUTHORIZED'
  ])('locks after %s without exposing backend text', async (code) => {
    put.mockResolvedValue({ success: false, code, error: 'secret /private/path' })
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(button('save'))
    await waitFor(() => expect(button('save')).toBeDisabled())
    expect(screen.queryByText('secret /private/path')).toBeNull()
    expect(check(en.hostAccess.enabled)).toBeDisabled()
  })
  it('tests and lists/cancels jobs with profileId/jobId and never renders job output', async () => {
    view.policy.enabled = true
    view.policy.profileFingerprints.p = 'fp-current'
    view.policy.ssh.profiles.p = {
      ...emptyMcpSshGrant(),
      operations: ['test_connection', 'job_list', 'job_cancel']
    }
    call.mockImplementation(async ({ tool }) => ({
      success: true,
      data:
        tool === 'job_list'
          ? {
              jobs: [
                { profileId: 'p', jobId: 'j', state: 'running', output: 'secret output' },
                { profileId: 'other', jobId: 'private', state: 'running' }
              ]
            }
          : {}
    }))
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(button('test'))
    await screen.findByText(en.hostAccess.tested)
    expect(call).toHaveBeenLastCalledWith({
      provider: 'ssh',
      tool: 'test_connection',
      arguments: { profileId: 'p' }
    })
    fireEvent.click(button('refreshJobs'))
    await screen.findByText('j · running')
    expect(screen.queryByText('secret output')).toBeNull()
    expect(screen.queryByText('private · running')).toBeNull()
    fireEvent.click(button('cancelJob'))
    await screen.findByText(en.hostAccess.cancelRequested)
    expect(call).toHaveBeenLastCalledWith({
      provider: 'ssh',
      tool: 'job_cancel',
      arguments: { profileId: 'p', jobId: 'j' }
    })
  })
  it('cannot test with an unsaved grant', async () => {
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(check(en.hostAccess.enabled))
    fireEvent.click(check(en.hostAccess.grantProfile))
    fireEvent.click(check(en.hostAccess.operations.test_connection))
    expect(button('test')).toBeDisabled()
    expect(call).not.toHaveBeenCalled()
  })

  it('preserves stale identity pins when editing other settings or disabling all access', async () => {
    view.policy.enabled = true
    view.policy.ssh.profiles.p = { ...emptyMcpSshGrant(), operations: ['test_connection'] }
    view.policy.profileFingerprints.p = 'fp-old'
    render(<McpHostAccessPanel />)
    await ready()
    expect(button('test')).toBeDisabled()
    fireEvent.click(check(en.hostAccess.workspaceEnabled))
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1].profileFingerprints).toEqual({ p: 'fp-old' })
    await waitFor(() => expect(button('save')).not.toBeDisabled())
    fireEvent.click(check(en.hostAccess.enabled))
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledTimes(2))
    expect(put.mock.calls[1][1]).toMatchObject({
      enabled: false,
      profileFingerprints: { p: 'fp-old' }
    })
  })

  it('requires a dedicated identity authorization action to replace a stale pin', async () => {
    view.policy.enabled = true
    view.policy.ssh.profiles.p = { ...emptyMcpSshGrant(), operations: ['test_connection'] }
    view.policy.profileFingerprints.p = 'fp-old'
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(button('reauthorizeIdentity'))
    expect(button('test')).toBeDisabled()
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1].profileFingerprints).toEqual({ p: 'fp-current' })
    await waitFor(() => expect(button('test')).not.toBeDisabled())
  })

  it('removes invisible grants for deleted profiles without refreshing retained identity pins', async () => {
    view.policy.ssh.profiles.deleted = { ...emptyMcpSshGrant(), operations: ['execute'] }
    view.policy.profileFingerprints.deleted = 'deleted-pin'
    view.policy.ssh.profiles.p = emptyMcpSshGrant()
    view.policy.profileFingerprints.p = 'fp-current'
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(check(en.hostAccess.enabled))
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    const saved = put.mock.calls[0][1]
    expect(saved.enabled).toBe(true)
    expect(saved.ssh.profiles).toEqual({ p: emptyMcpSshGrant() })
    expect(saved.profileFingerprints).toEqual({ p: 'fp-current' })
  })
})
