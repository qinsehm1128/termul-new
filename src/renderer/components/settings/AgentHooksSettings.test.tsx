import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { AgentHooksSettings } from './AgentHooksSettings'

const api = vi.hoisted(() => ({
  status: vi.fn(),
  preview: vi.fn(),
  install: vi.fn(),
  uninstall: vi.fn()
}))
vi.mock('@/lib/agent-hooks-api', () => ({ agentHooksApi: api }))
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (_key: string, options: { defaultValue: string }) => options.defaultValue
  })
}))
vi.mock('@/components/ConfirmDialog', () => ({
  ConfirmDialog: (props: {
    isOpen: boolean
    children: React.ReactNode
    message: string
    onConfirm: () => void
    onCancel: () => void
  }) =>
    props.isOpen ? (
      <div role="dialog">
        <p>{props.message}</p>
        {props.children}
        <button type="button" onClick={props.onConfirm}>
          Confirm install
        </button>
        <button type="button" onClick={props.onCancel}>
          Cancel
        </button>
      </div>
    ) : null
}))
const row = {
  agent: 'claude-code',
  state: 'not_installed',
  config_path: '/home/test/.claude/settings.json',
  detail: 'Preserves your existing hooks'
}

beforeEach(() => {
  vi.resetAllMocks()
  api.status.mockResolvedValue([row])
  api.preview.mockResolvedValue('--- settings.json\n+++ settings.json\n+ app-managed-hook')
  api.install.mockResolvedValue({ ...row, state: 'installed' })
  api.uninstall.mockResolvedValue(row)
})
afterEach(cleanup)

describe('AgentHooksSettings explicit host mutations', () => {
  it('only probes on mount and previews the exact host path before confirming installation', async () => {
    render(<AgentHooksSettings />)
    await screen.findByText(row.config_path)
    expect(api.install).not.toHaveBeenCalled()
    expect(api.uninstall).not.toHaveBeenCalled()
    expect(api.preview).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Install' }))
    await screen.findByRole('dialog')
    expect(api.preview).toHaveBeenCalledExactlyOnceWith('claude-code')
    expect(screen.getByRole('dialog').textContent).toContain('+ app-managed-hook')
    expect(screen.getByRole('dialog').textContent).toContain(row.config_path)
    expect(screen.getByRole('dialog').textContent).toContain('unrelated hooks are preserved')
    expect(api.install).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(api.install).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Install' }))
    await screen.findByRole('dialog')
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Confirm install' })))
    expect(api.install).toHaveBeenCalledExactlyOnceWith('claude-code')
    expect(api.status).toHaveBeenCalledTimes(2)
  })
  it('shows read-only View changes and never offers confirmation after preview failure', async () => {
    render(<AgentHooksSettings />)
    const view = await screen.findByRole('button', { name: 'View changes' })
    fireEvent.click(view)
    await screen.findByRole('dialog')
    expect(api.install).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    api.preview.mockRejectedValueOnce(new Error('cannot preview config'))
    fireEvent.click(view)
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toContain('cannot preview config')
    )
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(api.install).not.toHaveBeenCalled()
  })
  it('rejects an empty preview instead of allowing a blind install', async () => {
    api.preview.mockResolvedValue('  ')
    render(<AgentHooksSettings />)
    fireEvent.click(await screen.findByRole('button', { name: 'Install' }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('preview is empty'))
    expect(screen.queryByRole('button', { name: 'Confirm install' })).toBeNull()
    expect(api.install).not.toHaveBeenCalled()
  })
  it('offers reinstall/uninstall for an outdated installation and refreshes after uninstall', async () => {
    api.status.mockResolvedValue([{ ...row, state: 'outdated' }])
    render(<AgentHooksSettings />)
    await screen.findByRole('button', { name: 'Reinstall' })
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Uninstall' })))
    expect(api.uninstall).toHaveBeenCalledExactlyOnceWith('claude-code')
    expect(api.status).toHaveBeenCalledTimes(2)
  })
  it.each([
    undefined,
    null,
    { statuses: [] }
  ])('treats malformed old-host status %j as unavailable without startup mutations', async (response) => {
    api.status.mockResolvedValue(response)
    render(<AgentHooksSettings />)
    await waitFor(() => expect(screen.getAllByText('Unavailable')).toHaveLength(3))
    expect(api.status).toHaveBeenCalledOnce()
    expect(screen.queryByRole('button', { name: 'Install' })).toBeNull()
    expect(api.install).not.toHaveBeenCalled()
    expect(api.uninstall).not.toHaveBeenCalled()
    expect(api.preview).not.toHaveBeenCalled()
    api.status.mockResolvedValue([row])
    fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }))
    await screen.findByText(row.config_path)
    expect(api.install).not.toHaveBeenCalled()
  })
  it('reports errors without mutating or fabricating config paths', async () => {
    api.status.mockRejectedValue(new Error('remote unavailable'))
    render(<AgentHooksSettings />)
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toContain('remote unavailable')
    )
    expect(screen.queryByRole('button', { name: 'Install' })).toBeNull()
    expect(api.install).not.toHaveBeenCalled()
  })
  it('disables installation when the host cannot find the agent', async () => {
    api.status.mockResolvedValue([{ ...row, state: 'agent_not_found' }])
    render(<AgentHooksSettings />)
    expect(
      (await screen.findByRole('button', { name: 'Install' })) as HTMLButtonElement
    ).toHaveProperty('disabled', true)
  })
})
