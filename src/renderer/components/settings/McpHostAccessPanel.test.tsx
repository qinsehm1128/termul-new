import {
  emptyMcpHostAccessPolicy,
  emptyMcpSshGrant,
  MCP_SSH_OPERATIONS,
  type McpHostAccessView,
  type McpSshOperation
} from '@shared/types/mcp-host.types'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import en from '../../locales/en/mcp.json'
import { McpHostAccessPanel } from './McpHostAccessPanel'

const { get, put, call, preview } = vi.hoisted(() => ({
  get: vi.fn(),
  put: vi.fn(),
  call: vi.fn(),
  preview: vi.fn()
}))
vi.mock('@/lib/mcp-host-api', () => ({
  getMcpHostAccess: get,
  putMcpHostAccess: put,
  callMcpHost: call,
  previewMcpHostAccess: preview
}))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: translate }) }))
function translate(key: string, options: Record<string, string | number> = {}): string {
  const text =
    (key
      .split('.')
      .reduce<unknown>(
        (value, part) =>
          value && typeof value === 'object' ? (value as Record<string, unknown>)[part] : undefined,
        en
      ) as string) ?? key
  return text.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(options[name] ?? `{{${name}}}`))
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
const operationLabel = (op: McpSshOperation) =>
  `${en.hostAccess.operations[op]}${['execute', 'job_start', 'write_file', 'upload', 'download', 'job_cancel'].includes(op) ? ` — ${en.hostAccess.highRisk}` : ''}`
async function ready(expand = true): Promise<void> {
  await screen.findByRole('button', {
    name: translate('hostAccess.configureProfile', { name: 'Production' })
  })
  if (expand)
    fireEvent.click(
      screen.getByRole('button', {
        name: translate('hostAccess.configureProfile', { name: 'Production' })
      })
    )
}

describe('compact host access controls', () => {
  it('preserves global and profile rules and stale pins through defaults, readonly, roots and ordinary save', async () => {
    const globalRules = [{ id: 'g', kind: 'contains' as const, pattern: 'GLOBAL_RULE_SENTINEL' }]
    const profileRules = [
      { id: 'p-rule', kind: 'exact' as const, pattern: 'PROFILE_RULE_SENTINEL' }
    ]
    view.policy.ssh.commandDeny = globalRules
    view.policy.ssh.profiles.p = {
      ...emptyMcpSshGrant(),
      operations: ['read_file'],
      commandDeny: profileRules
    }
    view.policy.profileFingerprints.p = 'fp-old'
    view.profiles.push({ ...view.profiles[0], profileId: 'new-profile', name: 'New server' })
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(button('selectReadOnly'))
    fireEvent.change(screen.getByLabelText(en.hostAccess.remoteRoots), {
      target: { value: '/srv\n/srv' }
    })
    fireEvent.click(
      screen.getByRole('button', {
        name: translate('hostAccess.configureProfile', { name: 'New server' })
      })
    )
    // The second grant is all 13 operations; it must not erase global rules or repin p.
    fireEvent.click(screen.getAllByRole('checkbox', { name: en.hostAccess.grantProfile })[1])
    fireEvent.click(check(en.hostAccess.workspaceEnabled))
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    const saved = put.mock.calls[0][1]
    expect(saved.schemaVersion).toBe(2)
    expect(saved.ssh.commandDeny).toEqual(globalRules)
    expect(saved.ssh.profiles.p.commandDeny).toEqual(profileRules)
    expect(saved.ssh.profiles.p.remoteRoots).toEqual(['/srv'])
    expect(saved.ssh.profiles['new-profile'].operations).toEqual([...MCP_SSH_OPERATIONS])
    expect(saved.profileFingerprints.p).toBe('fp-old')
    expect(call).not.toHaveBeenCalled()
    expect(preview).not.toHaveBeenCalled()
  })
  it('adds global rules locally, shows immutable inheritance and saves without profile overrides', async () => {
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.change(screen.getByLabelText(en.commandDeny.newPattern), {
      target: { value: 'blocked text' }
    })
    fireEvent.click(screen.getByRole('button', { name: en.commandDeny.add }))
    // Inherited global rules show read-only inside a granted connection.
    fireEvent.click(check(en.hostAccess.grantProfile))
    const patterns = screen.getAllByLabelText(translate('commandDeny.rulePattern', { index: 1 }))
    expect(patterns).toHaveLength(2)
    expect(patterns[0]).not.toBeDisabled()
    expect(patterns[1]).toBeDisabled()
    expect(
      screen.getAllByRole('button', { name: translate('commandDeny.removeRule', { index: 1 }) })
    ).toHaveLength(1)
    expect(preview).not.toHaveBeenCalled()
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1].ssh.commandDeny).toEqual([
      expect.objectContaining({ kind: 'exact', pattern: 'blocked text' })
    ])
    expect(put.mock.calls[0][1].ssh.profiles.p.commandDeny).toEqual([])
  })
  it('loads legacy v1 without false dirty state and saves normalized v2', async () => {
    view.policy.schemaVersion = 1
    delete view.policy.ssh.commandDeny
    render(<McpHostAccessPanel />)
    await ready(false)
    expect(screen.queryByText(en.hostAccess.unsaved)).not.toBeInTheDocument()
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1]).toMatchObject({ schemaVersion: 2, ssh: { commandDeny: [] } })
  })
  it.each([
    'INVALID_ARGUMENT',
    'REVISION_CONFLICT'
  ])('retains rule drafts after save %s', async (code) => {
    view.policy.ssh.commandDeny = [{ id: 'g', kind: 'regex', pattern: '[' }]
    put.mockResolvedValue({ success: false, code, error: 'PRIVATE_BACKEND_ERROR' })
    render(<McpHostAccessPanel />)
    await ready(false)
    const input = screen.getAllByLabelText(translate('commandDeny.rulePattern', { index: 1 }))[0]
    fireEvent.change(input, { target: { value: 'changed[' } })
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(input).toHaveValue('changed[')
    expect(screen.queryByText('PRIVATE_BACKEND_ERROR')).not.toBeInTheDocument()
    expect(
      screen.getByText(
        code === 'REVISION_CONFLICT' ? en.hostAccess.conflict : en.commandDeny.invalidServer
      )
    ).toBeInTheDocument()
  })
  it('blocks invalid structural policy locally instead of saving or previewing', async () => {
    view.policy.ssh.commandDeny = [{ id: 'g', kind: 'exact', pattern: '' }]
    render(<McpHostAccessPanel />)
    await ready(false)
    fireEvent.click(button('save'))
    expect(put).not.toHaveBeenCalled()
    expect(preview).not.toHaveBeenCalled()
    expect(screen.getAllByText(en.commandDeny.invalid).length).toBeGreaterThan(0)
  })
  it('discards unsaved edits back to the saved policy without saving', async () => {
    render(<McpHostAccessPanel />)
    await ready(false)
    expect(button('discard')).toBeDisabled()
    fireEvent.click(check(en.hostAccess.grantProfile))
    expect(screen.getByText(en.hostAccess.unsaved)).toBeInTheDocument()
    fireEvent.click(button('discard'))
    expect(check(en.hostAccess.grantProfile)).not.toBeChecked()
    expect(screen.getByText(en.hostAccess.savedState)).toBeInTheDocument()
    expect(put).not.toHaveBeenCalled()
  })
  it('authorizing a new SSH profile selects all 13 operations without implicitly saving or expanding scope', async () => {
    render(<McpHostAccessPanel />)
    await ready(false)
    fireEvent.click(check(en.hostAccess.grantProfile))
    for (const op of MCP_SSH_OPERATIONS) expect(check(operationLabel(op))).toBeChecked()
    expect(check(en.hostAccess.enabled)).not.toBeChecked()
    expect(button('test')).toBeDisabled()
    expect(put).not.toHaveBeenCalled()
    expect(call).not.toHaveBeenCalled()
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1]).toMatchObject({
      enabled: false,
      workspace: { enabled: false, projectIds: [], includePaths: false },
      ssh: { profiles: { p: { ...emptyMcpSshGrant(), operations: [...MCP_SSH_OPERATIONS] } } },
      profileFingerprints: { p: 'fp-current' }
    })
  })

  it('keeps an existing restricted grant until explicitly revoked and authorized again', async () => {
    view.policy.ssh.profiles.p = { ...emptyMcpSshGrant(), operations: ['profile_get'] }
    view.policy.profileFingerprints.p = 'fp-current'
    render(<McpHostAccessPanel />)
    await ready()
    expect(check(operationLabel('execute'))).not.toBeChecked()
    expect(check(operationLabel('profile_get'))).toBeChecked()
    fireEvent.click(check(en.hostAccess.grantProfile))
    fireEvent.click(check(en.hostAccess.grantProfile))
    for (const op of MCP_SSH_OPERATIONS) expect(check(operationLabel(op))).toBeChecked()
    expect(put).not.toHaveBeenCalled()
    expect(call).not.toHaveBeenCalled()
  })
  it('read-only preset removes all previously granted active and high-risk permissions', async () => {
    view.policy.ssh.profiles.p = { ...emptyMcpSshGrant(), operations: [...MCP_SSH_OPERATIONS] }
    view.policy.profileFingerprints.p = 'fp-current'
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(button('selectReadOnly'))
    for (const op of [
      'execute',
      'job_start',
      'write_file',
      'upload',
      'download',
      'job_cancel',
      'test_connection'
    ] as const)
      expect(check(operationLabel(op))).not.toBeChecked()
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1].ssh.profiles.p.operations).toEqual([
      'profile_list',
      'profile_get',
      'list_files',
      'read_file',
      'job_status',
      'job_list'
    ])
  })
  it('selects only matching projects and keeps selections outside the filter without future grants', async () => {
    view.policy.workspace.projectIds = ['b']
    render(<McpHostAccessPanel />)
    await ready(false)
    fireEvent.change(screen.getByRole('searchbox', { name: en.hostAccess.searchProjects }), {
      target: { value: 'Alpha' }
    })
    fireEvent.click(button('selectResults'))
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1]).toMatchObject({
      enabled: false,
      workspace: { enabled: false, projectIds: ['b', 'a'], includePaths: false }
    })
    expect(call).not.toHaveBeenCalled()
  })

  it('selects current projects, clears selections and disables select-all for an empty result', async () => {
    render(<McpHostAccessPanel />)
    await ready(false)
    expect(screen.getByRole('group', { name: en.hostAccess.projectChoices })).toBeVisible()
    fireEvent.click(button('selectResults'))
    expect(check('Alpha')).toBeChecked()
    expect(check('Beta')).toBeChecked()
    expect(check(en.hostAccess.allProjects)).not.toBeChecked()
    fireEvent.click(button('clearSelection'))
    expect(check('Alpha')).not.toBeChecked()
    expect(check('Beta')).not.toBeChecked()
    fireEvent.change(screen.getByRole('searchbox', { name: en.hostAccess.searchProjects }), {
      target: { value: 'missing' }
    })
    expect(button('selectResults')).toBeDisabled()
    expect(screen.getByText(en.hostAccess.noMatches)).toBeInTheDocument()
    expect(put).not.toHaveBeenCalled()
  })

  it('collapses sections and all SSH cards without changing grants or losing project choices', async () => {
    render(<McpHostAccessPanel />)
    await ready(false)
    expect(screen.queryByRole('button', { name: en.hostAccess.test })).toBeNull()
    fireEvent.click(check('Alpha'))
    const section = screen.getByRole('button', { name: /Workspace catalog/ })
    fireEvent.click(section)
    expect(section).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByRole('checkbox', { name: 'Alpha' })).toBeNull()
    fireEvent.click(section)
    expect(check('Alpha')).toBeChecked()
    fireEvent.click(button('expandAll'))
    expect(button('test')).toBeDisabled()
    fireEvent.click(button('collapseAll'))
    expect(screen.queryByRole('button', { name: en.hostAccess.test })).toBeNull()
    expect(check(en.hostAccess.grantProfile)).not.toBeChecked()
    expect(call).not.toHaveBeenCalled()
  })

  it('offers read-only selection and requires explicit confirmation for all high-risk operations', async () => {
    render(<McpHostAccessPanel />)
    await ready(false)
    fireEvent.click(check(en.hostAccess.grantProfile))
    fireEvent.click(button('selectReadOnly'))
    const read = [
      'profile_list',
      'profile_get',
      'list_files',
      'read_file',
      'job_status',
      'job_list'
    ]
    for (const op of MCP_SSH_OPERATIONS)
      expect(check(operationLabel(op)).getAttribute('type')).toBe('checkbox')
    for (const op of MCP_SSH_OPERATIONS)
      expect((check(operationLabel(op)) as HTMLInputElement).checked).toBe(read.includes(op))
    fireEvent.click(button('selectAllPermissions'))
    expect(check(operationLabel('execute'))).not.toBeChecked()
    fireEvent.click(button('cancelSelection'))
    expect(screen.queryByRole('button', { name: en.hostAccess.confirmAllPermissions })).toBeNull()
    fireEvent.click(button('selectAllPermissions'))
    fireEvent.click(button('confirmAllPermissions'))
    for (const op of MCP_SSH_OPERATIONS) expect(check(operationLabel(op))).toBeChecked()
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1].ssh.profiles.p).toEqual({
      ...emptyMcpSshGrant(),
      operations: [...MCP_SSH_OPERATIONS]
    })
    expect(put.mock.calls[0][1].enabled).toBe(false)
    expect(call).not.toHaveBeenCalled()
  })

  it('bulk permission edits never refresh a changed profile identity', async () => {
    view.policy.ssh.profiles.p = emptyMcpSshGrant()
    view.policy.profileFingerprints.p = 'old-pin'
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(button('selectAllPermissions'))
    fireEvent.click(button('confirmAllPermissions'))
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1].profileFingerprints.p).toBe('old-pin')
  })

  it('searching SSH profiles does not hide grants from the saved policy', async () => {
    view.profiles.push({
      ...view.profiles[0],
      profileId: 'other',
      name: 'Backup',
      host: 'backup.invalid',
      fingerprint: 'backup-pin'
    })
    view.policy.ssh.profiles.other = { ...emptyMcpSshGrant(), operations: ['profile_get'] }
    view.policy.profileFingerprints.other = 'backup-pin'
    render(<McpHostAccessPanel />)
    await ready(false)
    fireEvent.change(screen.getByRole('searchbox', { name: en.hostAccess.searchProfiles }), {
      target: { value: 'Production' }
    })
    expect(
      screen.queryByRole('button', {
        name: translate('hostAccess.configureProfile', { name: 'Backup' })
      })
    ).not.toBeInTheDocument()
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    expect(put.mock.calls[0][1].ssh.profiles.other.operations).toEqual(['profile_get'])
    expect(put.mock.calls[0][1].profileFingerprints.other).toBe('backup-pin')
  })

  it('blocks selection shortcuts on PublicTunnel while disclosure remains usable', async () => {
    view.hostMode = 'PUBLIC_TUNNEL'
    view.policy.ssh.profiles.p = emptyMcpSshGrant()
    view.policy.profileFingerprints.p = 'fp-current'
    render(<McpHostAccessPanel />)
    await ready()
    expect(button('selectResults')).toBeDisabled()
    expect(button('selectReadOnly')).toBeDisabled()
    expect(button('selectAllPermissions')).toBeDisabled()
    expect(button('expandAll')).not.toBeDisabled()
    expect(put).not.toHaveBeenCalled()
    expect(call).not.toHaveBeenCalled()
  })
})

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
    expect(screen.getByText(en.hostAccess.grantToConfigure)).toBeInTheDocument()
    expect(container.querySelector('input[type=password]')).toBeNull()
    expect(call).not.toHaveBeenCalled()
  })
  it('treats checked operations as the whole grant: no roots or job switch are required', async () => {
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(check(en.hostAccess.enabled))
    fireEvent.click(check(en.hostAccess.grantProfile))
    expect(screen.queryByRole('checkbox', { name: /POSIX/ })).toBeNull()
    expect(screen.getByText(en.hostAccess.pathLimitTitle)).toBeInTheDocument()
    expect(screen.queryByText(en.hostAccess.pathLimitActive)).toBeNull()
    fireEvent.click(button('save'))
    await waitFor(() => expect(put).toHaveBeenCalledOnce())
    const saved = put.mock.calls[0][1].ssh.profiles.p
    expect(saved.operations).toHaveLength(MCP_SSH_OPERATIONS.length)
    expect(saved.remoteRoots).toEqual([])
    expect(saved.localRoots).toEqual([])
  })
  it('saves selected project IDs, individual SSH grants, optional roots and fingerprints', async () => {
    render(<McpHostAccessPanel />)
    await ready()
    fireEvent.click(check(en.hostAccess.enabled))
    fireEvent.click(check(en.hostAccess.workspaceEnabled))
    fireEvent.click(check('Alpha'))
    fireEvent.click(check(en.hostAccess.includePaths))
    fireEvent.click(check(en.hostAccess.grantProfile))
    for (const op of MCP_SSH_OPERATIONS) expect(check(operationLabel(op))).toBeChecked()
    fireEvent.click(button('clearPermissions'))
    for (const op of MCP_SSH_OPERATIONS) expect(check(operationLabel(op))).not.toBeChecked()
    fireEvent.click(check(operationLabel('execute')))
    fireEvent.click(check(`${en.hostAccess.operations.upload} — ${en.hostAccess.highRisk}`))
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
        schemaVersion: 2,
        revision: 0,
        enabled: true,
        workspace: { enabled: true, projectIds: ['a'], includePaths: true },
        ssh: {
          commandDeny: [],
          profiles: {
            p: {
              commandDeny: [],
              operations: ['execute', 'upload'],
              remoteRoots: ['/srv', '/opt'],
              localRoots: ['/host-data'],
              posixJobs: false
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
    expect(check(operationLabel('test_connection'))).toBeChecked()
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
