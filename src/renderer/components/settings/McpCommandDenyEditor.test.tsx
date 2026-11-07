import type { IpcResult } from '@shared/types/ipc.types'
import {
  emptyMcpHostAccessPolicy,
  emptyMcpSshGrant,
  type McpCommandDenyRule,
  type McpHostAccessPreviewResult
} from '@shared/types/mcp-host.types'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  McpCommandDenyEditor,
  validCommandDenyPolicy,
  validCommandDenyRules
} from './McpCommandDenyEditor'
import { McpCommandDenyPreview } from './McpCommandDenyPreview'

const api = vi.hoisted(() => ({
  previewMcpHostAccess: vi.fn(),
  putMcpHostAccess: vi.fn(),
  callMcpHost: vi.fn()
}))
vi.mock('@/lib/mcp-host-api', () => api)
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, options?: { index?: number }) =>
      options?.index ? `${key} ${options.index}` : key
  })
}))
afterEach(cleanup)
beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
})
const rule = (id = 'r', pattern = 'rm -rf'): McpCommandDenyRule => ({ id, pattern, kind: 'exact' })
const profiles = [
  {
    profileId: 'p',
    name: 'Production',
    fingerprint: 'pin',
    host: 'test',
    port: 22,
    username: 'user',
    authMethod: 'agent',
    tags: []
  }
]

function StatefulEditor({ initial = [] }: { initial?: McpCommandDenyRule[] }) {
  const [rules, setRules] = useState(initial)
  return <McpCommandDenyEditor title="Rules" rules={rules} onChange={setRules} />
}
function setupPreview() {
  const policy = emptyMcpHostAccessPolicy()
  render(<McpCommandDenyPreview policy={policy} profiles={profiles} dirty />)
  fireEvent.change(screen.getByLabelText('commandDeny.previewProfile'), { target: { value: 'p' } })
  fireEvent.change(screen.getByLabelText('commandDeny.command'), {
    target: { value: 'COMMAND_SENTINEL' }
  })
  return policy
}
const previewResult: McpHostAccessPreviewResult = {
  outcome: 'not_matched',
  matches: [],
  policyRevision: 7,
  draft: true
}

describe('MCP command deny editor', () => {
  it('defaults to literal matching, adds, edits and removes locally without any calls', () => {
    render(<StatefulEditor />)
    expect(screen.getByLabelText('commandDeny.newKind')).toHaveValue('exact')
    expect(screen.queryByRole('option', { name: 'commandDeny.regex' })).not.toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('commandDeny.newPattern'), {
      target: { value: '.*[literal]' }
    })
    fireEvent.click(screen.getByRole('button', { name: 'commandDeny.add' }))
    expect(screen.getByLabelText('commandDeny.rulePattern 1')).toHaveValue('.*[literal]')
    fireEvent.change(screen.getByLabelText('commandDeny.rulePattern 1'), {
      target: { value: 'new text' }
    })
    fireEvent.click(screen.getByRole('button', { name: 'commandDeny.removeRule 1' }))
    expect(screen.queryByLabelText('commandDeny.rulePattern 1')).not.toBeInTheDocument()
    expect(api.previewMcpHostAccess).not.toHaveBeenCalled()
    expect(api.putMcpHostAccess).not.toHaveBeenCalled()
    expect(api.callMcpHost).not.toHaveBeenCalled()
  })
  it('exposes regex only as advanced and does not compile it in JavaScript', () => {
    render(<StatefulEditor />)
    fireEvent.click(screen.getByLabelText('commandDeny.advanced'))
    fireEvent.change(screen.getByLabelText('commandDeny.newKind'), { target: { value: 'regex' } })
    fireEvent.change(screen.getByLabelText('commandDeny.newPattern'), { target: { value: '[' } })
    fireEvent.click(screen.getByRole('button', { name: 'commandDeny.add' }))
    expect(screen.getByLabelText('commandDeny.ruleKind 1')).toHaveValue('regex')
    expect(validCommandDenyRules([{ ...rule(), kind: 'regex', pattern: '[' }])).toBe(true)
    expect(api.previewMcpHostAccess).not.toHaveBeenCalled()
  })
  it('makes inherited global rules immutable without remove/add controls', () => {
    const change = vi.fn()
    render(<McpCommandDenyEditor title="Inherited" rules={[rule()]} inherited onChange={change} />)
    expect(screen.getByLabelText('commandDeny.rulePattern 1')).toBeDisabled()
    expect(screen.getByLabelText('commandDeny.ruleKind 1')).toBeDisabled()
    expect(screen.queryByRole('button')).not.toBeInTheDocument()
    expect(screen.getByText('commandDeny.inherited')).toBeInTheDocument()
    expect(change).not.toHaveBeenCalled()
  })
  it('enforces per-layer and UTF-8 byte limits with a local error', () => {
    expect(validCommandDenyRules([rule('a', 'x'.repeat(512))])).toBe(true)
    expect(validCommandDenyRules([rule('a', '界'.repeat(171))])).toBe(false)
    expect(validCommandDenyRules([rule('a', ' ')])).toBe(true)
    expect(validCommandDenyRules([rule('a', '')])).toBe(false)
    expect(validCommandDenyRules([rule('a', 'x\0y')])).toBe(false)
    expect(validCommandDenyRules([rule('invalid id')])).toBe(false)
    expect(validCommandDenyRules([rule('界')])).toBe(false)
    expect(validCommandDenyRules([rule('a'.repeat(129))])).toBe(false)
    expect(validCommandDenyRules([rule('a'), rule('a')])).toBe(false)
    render(<StatefulEditor initial={Array.from({ length: 32 }, (_, i) => rule(`${i}`))} />)
    fireEvent.change(screen.getByLabelText('commandDeny.newPattern'), { target: { value: 'x' } })
    expect(screen.getByRole('button', { name: 'commandDeny.add' })).toBeDisabled()
    expect(validCommandDenyRules(Array.from({ length: 33 }, (_, i) => rule(`${i}`)))).toBe(false)
  })
  it('checks total rules across global and all profiles, but allows inherited IDs in separate layers', () => {
    const policy = emptyMcpHostAccessPolicy()
    policy.ssh.commandDeny = Array.from({ length: 32 }, (_, i) => rule(`${i}`))
    for (let i = 0; i < 3; i++)
      policy.ssh.profiles[`${i}`] = {
        ...emptyMcpSshGrant(),
        commandDeny: [...policy.ssh.commandDeny]
      }
    expect(validCommandDenyPolicy(policy)).toBe(true)
    policy.ssh.profiles.extra = { ...emptyMcpSshGrant(), commandDeny: [rule()] }
    expect(validCommandDenyPolicy(policy)).toBe(false)
  })
  it('disables controls in readonly or busy context', () => {
    render(<McpCommandDenyEditor title="Rules" rules={[rule()]} onChange={vi.fn()} disabled />)
    expect(screen.getByLabelText('commandDeny.newPattern')).toBeDisabled()
    expect(screen.getByLabelText('commandDeny.rulePattern 1')).toBeDisabled()
  })
})

describe('pure MCP command deny preview', () => {
  it('never auto previews, saves or executes and uses explicit draft input on click', async () => {
    api.previewMcpHostAccess.mockResolvedValue({ success: true, data: previewResult })
    const policy = setupPreview()
    expect(api.previewMcpHostAccess).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'commandDeny.preview' }))
    await screen.findByText('commandDeny.notMatched')
    expect(api.previewMcpHostAccess).toHaveBeenCalledExactlyOnceWith({
      profileId: 'p',
      operation: 'execute',
      command: 'COMMAND_SENTINEL',
      policy
    })
    expect(screen.getByText('commandDeny.draftResult')).toBeInTheDocument()
    expect(screen.getByText('commandDeny.notPermission')).toBeInTheDocument()
    expect(api.putMcpHostAccess).not.toHaveBeenCalled()
    expect(api.callMcpHost).not.toHaveBeenCalled()
    expect(JSON.stringify(localStorage)).not.toContain('COMMAND_SENTINEL')
  })
  it('omits draft for effective-policy preview and shows server matches by source', async () => {
    api.previewMcpHostAccess.mockResolvedValue({
      success: true,
      data: {
        ...previewResult,
        draft: false,
        outcome: 'matched',
        matches: [
          { id: 'global-id', source: 'global' },
          { id: 'profile-id', source: 'profile' }
        ],
        blockedBy: 'operation_not_granted'
      }
    })
    setupPreview()
    fireEvent.click(screen.getByLabelText('commandDeny.useDraft'))
    fireEvent.change(screen.getByLabelText('commandDeny.previewOperation'), {
      target: { value: 'job_start' }
    })
    fireEvent.click(screen.getByRole('button', { name: 'commandDeny.preview' }))
    await screen.findByText('commandDeny.matched')
    expect(api.previewMcpHostAccess).toHaveBeenCalledExactlyOnceWith({
      profileId: 'p',
      operation: 'job_start',
      command: 'COMMAND_SENTINEL'
    })
    expect(screen.getByText('commandDeny.activeResult')).toBeInTheDocument()
    expect(screen.getByText('global-id')).toBeInTheDocument()
    expect(screen.getByText('profile-id')).toBeInTheDocument()
    expect(screen.getByText('commandDeny.blocked')).toBeInTheDocument()
  })
  it.each([
    'INVALID_ARGUMENT',
    'REVISION_CONFLICT'
  ])('preserves command and draft on %s without reflecting backend error', async (code) => {
    api.previewMcpHostAccess.mockResolvedValue({
      success: false,
      code,
      error: 'ERROR_COMMAND_SENTINEL'
    })
    setupPreview()
    fireEvent.click(screen.getByRole('button', { name: 'commandDeny.preview' }))
    await screen.findByRole('alert')
    expect(screen.getByLabelText('commandDeny.command')).toHaveValue('COMMAND_SENTINEL')
    expect(screen.queryByText('ERROR_COMMAND_SENTINEL')).not.toBeInTheDocument()
    expect(api.putMcpHostAccess).not.toHaveBeenCalled()
  })
  it('blocks invalid drafts, missing profile and blank command, and clears command when switching profiles', () => {
    const policy = emptyMcpHostAccessPolicy()
    policy.ssh.commandDeny = [rule('a', '')]
    render(<McpCommandDenyPreview policy={policy} profiles={profiles} dirty />)
    expect(screen.getByRole('button', { name: 'commandDeny.preview' })).toBeDisabled()
    fireEvent.change(screen.getByLabelText('commandDeny.previewProfile'), {
      target: { value: 'p' }
    })
    fireEvent.change(screen.getByLabelText('commandDeny.command'), { target: { value: 'cmd' } })
    expect(screen.getByRole('button', { name: 'commandDeny.preview' })).toBeDisabled()
    fireEvent.click(screen.getByLabelText('commandDeny.useDraft'))
    expect(screen.getByRole('button', { name: 'commandDeny.preview' })).not.toBeDisabled()
    fireEvent.change(screen.getByLabelText('commandDeny.previewProfile'), { target: { value: '' } })
    expect(screen.getByLabelText('commandDeny.command')).toHaveValue('')
    expect(api.previewMcpHostAccess).not.toHaveBeenCalled()
  })
  it('invalidates in-flight results when policy changes and prevents duplicate requests', async () => {
    let resolve!: (result: IpcResult<McpHostAccessPreviewResult>) => void
    api.previewMcpHostAccess.mockReturnValue(
      new Promise((done) => {
        resolve = done
      })
    )
    const policy = emptyMcpHostAccessPolicy()
    const { rerender } = render(<McpCommandDenyPreview policy={policy} profiles={profiles} dirty />)
    fireEvent.change(screen.getByLabelText('commandDeny.previewProfile'), {
      target: { value: 'p' }
    })
    fireEvent.change(screen.getByLabelText('commandDeny.command'), { target: { value: 'cmd' } })
    fireEvent.click(screen.getByRole('button', { name: 'commandDeny.preview' }))
    expect(screen.getByLabelText('commandDeny.command')).toBeDisabled()
    expect(screen.getByRole('button', { name: 'commandDeny.previewing' })).toBeDisabled()
    rerender(
      <McpCommandDenyPreview policy={{ ...policy, revision: 8 }} profiles={profiles} dirty />
    )
    await act(async () => resolve({ success: true, data: previewResult }))
    await waitFor(() =>
      expect(screen.queryByText('commandDeny.notMatched')).not.toBeInTheDocument()
    )
  })
  it('disables preview in PublicTunnel/readonly/busy context', () => {
    render(
      <McpCommandDenyPreview
        policy={emptyMcpHostAccessPolicy()}
        profiles={profiles}
        dirty={false}
        disabled
      />
    )
    expect(screen.getByLabelText('commandDeny.command')).toBeDisabled()
    expect(screen.getByLabelText('commandDeny.useDraft')).toBeDisabled()
    expect(api.previewMcpHostAccess).not.toHaveBeenCalled()
  })
})
