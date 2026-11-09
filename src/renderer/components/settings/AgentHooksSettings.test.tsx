import type {
  AdapterStatus,
  AdapterStatusDto,
  HookMutationResult,
  HookPreviewDto
} from '@shared/types/agent-hooks.types'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import enSettings from '@/locales/en/settings.json'
import zhSettings from '@/locales/zh-CN/settings.json'
import { AgentHooksSettings } from './AgentHooksSettings'

describe('hook settings translations', () => {
  it('ships matching English/Chinese hook keys and localized safety copy', () => {
    expect(Object.keys(zhSettings.agentHooks).sort()).toEqual(
      Object.keys(enSettings.agentHooks).sort()
    )
    expect(Object.keys(zhSettings.agentHooks.states).sort()).toEqual(
      Object.keys(enSettings.agentHooks.states).sort()
    )
    expect(zhSettings.agentHooks.confirmUninstall).toBe('确认卸载')
    expect(zhSettings.agentHooks.uninstallConsequence).toContain('恢复原始配置')
    expect(enSettings.agentHooks.uninstallConsequence).toContain(
      'restore the original configuration'
    )
    expect(zhSettings.agentHooks.source.custom).toBe('自定义')
  })
})

const mocks = vi.hoisted(() => ({
  api: {
    status: vi.fn(),
    reload: vi.fn(),
    probe: vi.fn(),
    preview: vi.fn(),
    confirm: vi.fn(),
    install: vi.fn(),
    uninstall: vi.fn(),
    openFolder: vi.fn()
  },
  desktop: vi.fn(),
  log: vi.fn(),
  t: (_key: string, options: { defaultValue: string }) => options.defaultValue
}))
vi.mock('@/lib/agent-hooks-api', () => ({ agentHooksApi: mocks.api }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: mocks.log }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: mocks.desktop }))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: mocks.t }) }))

const row: AdapterStatusDto = {
  id: 'custom-agent',
  display_name: 'Custom Agent',
  version: '3.0.0',
  origin: 'custom',
  state: 'not_installed',
  installed_version: null,
  install_id: null,
  capabilities: { chat: false, resume: true, events: [] },
  diagnostics: []
}
const preview: HookPreviewDto = {
  adapter_id: row.id,
  action: 'install',
  preview_id: 'opaque-preview-id',
  fingerprint: 'host-fingerprint',
  files: [
    {
      path: '/home/test/.agent/hooks.sh',
      before: null,
      after: '#!/bin/sh\nreport-hook',
      mode: '0700'
    }
  ],
  keys: [
    {
      path: '/home/test/.agent/settings.json',
      key: ['hooks', 'SessionStart'],
      before: '',
      after: '{"command":"managed-hook"}'
    }
  ],
  capabilities: row.capabilities,
  diagnostics: []
}
const installed: AdapterStatusDto = {
  ...row,
  state: 'installed',
  installed_version: row.version,
  install_id: 'install-1'
}
const mutation: HookMutationResult = { status: installed, receipt_revision: 1 }
const api = mocks.api

beforeEach(() => {
  vi.resetAllMocks()
  mocks.desktop.mockReturnValue(false)
  mocks.log.mockResolvedValue(undefined)
  api.reload.mockResolvedValue([row])
  api.preview.mockResolvedValue(preview)
  api.install.mockResolvedValue(mutation)
  api.uninstall.mockResolvedValue({ status: row, receipt_revision: 1 } satisfies HookMutationResult)
  api.openFolder.mockResolvedValue(undefined)
})
afterEach(cleanup)

async function openInstall() {
  fireEvent.click(await screen.findByRole('button', { name: 'Install Custom Agent' }))
  return screen.findByRole('alertdialog', { name: 'Review hook changes · Custom Agent' })
}

function expectNoMutation() {
  expect(api.install).not.toHaveBeenCalled()
  expect(api.uninstall).not.toHaveBeenCalled()
  expect(api.confirm).not.toHaveBeenCalled()
  expect(api.probe).not.toHaveBeenCalled()
}

describe('AgentHooksSettings dynamic adapters', () => {
  it('reloads definitions only on mount and on Reload; never probes or installs implicitly', async () => {
    render(<AgentHooksSettings />)
    await screen.findByRole('article', { name: row.display_name })
    expect(api.reload).toHaveBeenCalledExactlyOnceWith()
    expect(api.status).not.toHaveBeenCalled()
    expect(api.preview).not.toHaveBeenCalled()
    expectNoMutation()
    api.reload.mockResolvedValueOnce([{ ...row, id: 'new-adapter', display_name: 'New Adapter' }])
    fireEvent.click(screen.getByRole('button', { name: 'Reload' }))
    await screen.findByRole('article', { name: 'New Adapter' })
    expect(screen.queryByRole('article', { name: row.display_name })).toBeNull()
    expect(api.reload).toHaveBeenCalledTimes(2)
    expectNoMutation()
  })

  it('renders arbitrary builtin/custom adapters, all six statuses and inline diagnostics', async () => {
    const states: AdapterStatus[] = [
      'not_installed',
      'installed',
      'outdated',
      'conflict',
      'source_missing',
      'invalid'
    ]
    api.reload.mockResolvedValue(
      states.map(
        (state, index) =>
          ({
            ...row,
            id: `adapter-${state}`,
            display_name: `Adapter ${index}`,
            state,
            origin: index === 0 ? 'builtin' : 'custom',
            diagnostics: [{ code: `DIAGNOSTIC_${index}`, message: `Explanation ${index}` }]
          }) satisfies AdapterStatusDto
      )
    )
    render(<AgentHooksSettings />)
    const cards = await screen.findAllByRole('article')
    expect(cards).toHaveLength(6)
    for (const [index, label] of [
      'Not installed',
      'Installed',
      'Outdated',
      'Conflict',
      'Source missing',
      'Invalid'
    ].entries()) {
      expect(within(cards[index]).getByText(label)).toBeTruthy()
      expect(within(cards[index]).getByText(`Explanation ${index}`, { exact: false })).toBeTruthy()
      expect(within(cards[index]).getByText(`DIAGNOSTIC_${index}`)).toBeTruthy()
    }
    expect(screen.getByText('Built-in')).toBeTruthy()
    expect(screen.getAllByText('Custom')).toHaveLength(5)
    expect(
      (screen.getByRole('button', { name: 'Install Adapter 5' }) as HTMLButtonElement).disabled
    ).toBe(true)
    expectNoMutation()
  })

  it('shows a concise help trigger instead of persistent instructional copy', async () => {
    render(<AgentHooksSettings />)
    await screen.findByRole('article')
    expect(screen.getByRole('button', { name: 'About agent hooks' })).toBeTruthy()
    expect(screen.queryByText(/Hooks report terminal activity/)).toBeNull()
  })

  it('shows no fabricated builtin rows for an empty host response', async () => {
    api.reload.mockResolvedValueOnce([])
    render(<AgentHooksSettings />)
    await screen.findByText('No adapters')
    expect(screen.queryByRole('article')).toBeNull()
    expectNoMutation()
  })

  it('exposes Open folder only on desktop and calls the facade without paths', async () => {
    const view = render(<AgentHooksSettings />)
    await screen.findByRole('article')
    expect(screen.queryByRole('button', { name: 'Open folder' })).toBeNull()
    view.unmount()
    mocks.desktop.mockReturnValue(true)
    render(<AgentHooksSettings />)
    await screen.findByRole('article')
    fireEvent.click(screen.getByRole('button', { name: 'Open folder' }))
    await waitFor(() => expect(api.openFolder).toHaveBeenCalledExactlyOnceWith())
    expectNoMutation()
  })
})

describe('AgentHooksSettings explicit preview and confirm', () => {
  it('shows exact files, keys, modes and contents before confirming an install', async () => {
    render(<AgentHooksSettings />)
    const dialog = await openInstall()
    expect(api.preview).toHaveBeenCalledExactlyOnceWith({
      adapter_id: row.id,
      action: 'install',
      event_options: {}
    })
    for (const content of [
      preview.files[0].path,
      preview.keys[0].path,
      '["hooks","SessionStart"]',
      '0700',
      'Absent',
      'Empty',
      preview.files[0].after!,
      preview.keys[0].after!
    ]) {
      expect(dialog.textContent).toContain(content)
    }
    expect(within(dialog).getByLabelText('Capabilities').textContent).toContain('ChatUnavailable')
    expect(within(dialog).getByLabelText('Capabilities').textContent).toContain('ResumeAvailable')
    expectNoMutation()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm install' }))
    await waitFor(() =>
      expect(api.install).toHaveBeenCalledExactlyOnceWith({
        adapter_id: row.id,
        preview_id: preview.preview_id,
        fingerprint: preview.fingerprint
      })
    )
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expect(screen.getByText('Installed', { exact: true })).toBeTruthy()
    expect(api.reload).toHaveBeenCalledOnce()
    expect(mocks.log).not.toHaveBeenCalled()
  })

  it('cancels installation without mutation and restores keyboard focus', async () => {
    render(<AgentHooksSettings />)
    const dialog = await openInstall()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Install Custom Agent' })
      )
    )
    expectNoMutation()
  })

  it('upgrades explicitly using an install preview and a token-only confirmation', async () => {
    api.reload.mockResolvedValueOnce([
      { ...installed, state: 'outdated', installed_version: '2.0.0' }
    ])
    render(<AgentHooksSettings />)
    fireEvent.click(await screen.findByRole('button', { name: 'Upgrade Custom Agent' }))
    const dialog = await screen.findByRole('alertdialog')
    expect(api.preview).toHaveBeenCalledExactlyOnceWith({
      adapter_id: row.id,
      action: 'install',
      event_options: {}
    })
    expectNoMutation()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm install' }))
    await waitFor(() =>
      expect(api.install).toHaveBeenCalledExactlyOnceWith({
        adapter_id: row.id,
        preview_id: preview.preview_id,
        fingerprint: preview.fingerprint
      })
    )
  })

  it('previews uninstall, explains its consequence, supports cancel, then confirms', async () => {
    api.reload.mockResolvedValueOnce([installed])
    api.preview.mockResolvedValue({
      ...preview,
      action: 'uninstall',
      files: [{ ...preview.files[0], before: preview.files[0].after, after: null }]
    } satisfies HookPreviewDto)
    render(<AgentHooksSettings />)
    fireEvent.click(await screen.findByRole('button', { name: 'Uninstall Custom Agent' }))
    let dialog = await screen.findByRole('alertdialog')
    expect(dialog.textContent).toContain('New hook activity will no longer be reported')
    expect(dialog.textContent).toContain('restore the original configuration')
    expect(api.preview).toHaveBeenCalledExactlyOnceWith({
      adapter_id: row.id,
      action: 'uninstall',
      event_options: {}
    })
    expectNoMutation()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expectNoMutation()
    fireEvent.click(screen.getByRole('button', { name: 'Uninstall Custom Agent' }))
    dialog = await screen.findByRole('alertdialog')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm uninstall' }))
    await waitFor(() =>
      expect(api.uninstall).toHaveBeenCalledExactlyOnceWith({
        adapter_id: row.id,
        preview_id: preview.preview_id,
        fingerprint: preview.fingerprint
      })
    )
    await screen.findByText('Not installed', { exact: true })
  })

  it('permits uninstall from an installed snapshot when a source is missing', async () => {
    api.reload.mockResolvedValueOnce([{ ...installed, version: null, state: 'source_missing' }])
    api.preview.mockResolvedValueOnce({ ...preview, action: 'uninstall' })
    render(<AgentHooksSettings />)
    const uninstall = await screen.findByRole('button', { name: 'Uninstall Custom Agent' })
    expect((uninstall as HTMLButtonElement).disabled).toBe(false)
    expect(
      (screen.getByRole('button', { name: 'Install Custom Agent' }) as HTMLButtonElement).disabled
    ).toBe(true)
    fireEvent.click(uninstall)
    await screen.findByRole('alertdialog')
    expectNoMutation()
  })

  it('renders file content as text, not executable markup', async () => {
    api.preview.mockResolvedValueOnce({
      ...preview,
      files: [{ ...preview.files[0], after: '<img src=x onerror="alert(1)">' }]
    } satisfies HookPreviewDto)
    render(<AgentHooksSettings />)
    const dialog = await openInstall()
    expect(dialog.textContent).toContain('<img src=x onerror="alert(1)">')
    expect(dialog.querySelector('img')).toBeNull()
  })

  it.each([
    { ...preview, files: [], keys: [] },
    { ...preview, adapter_id: 'different-agent' },
    { ...preview, action: 'uninstall' },
    { ...preview, fingerprint: '' }
  ])('does not offer confirmation for an empty or mismatched preview %#', async (invalid) => {
    api.preview.mockResolvedValueOnce(invalid)
    render(<AgentHooksSettings />)
    fireEvent.click(await screen.findByRole('button', { name: 'Install Custom Agent' }))
    await screen.findByRole('alert')
    expect(screen.queryByRole('alertdialog')).toBeNull()
    expectNoMutation()
  })

  it('locks confirmation while a mutation is in flight', async () => {
    let resolve!: (value: HookMutationResult) => void
    api.install.mockReturnValueOnce(
      new Promise<HookMutationResult>((done) => {
        resolve = done
      })
    )
    render(<AgentHooksSettings />)
    const dialog = await openInstall()
    const button = within(dialog).getByRole('button', { name: 'Confirm install' })
    fireEvent.click(button)
    fireEvent.click(button)
    expect(api.install).toHaveBeenCalledOnce()
    expect((button as HTMLButtonElement).disabled).toBe(true)
    expect(
      (within(dialog).getByRole('button', { name: 'Cancel' }) as HTMLButtonElement).disabled
    ).toBe(true)
    await act(async () => resolve(mutation))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
  })
})

describe('AgentHooksSettings boundary failures', () => {
  it('shows a reload failure, logs no raw payload, and allows a manual retry', async () => {
    api.reload.mockRejectedValueOnce(new Error('/private/path SECRET'))
    render(<AgentHooksSettings />)
    await screen.findByRole('alert')
    expect(screen.queryByRole('article')).toBeNull()
    expect(screen.getByRole('alert').textContent).toBe('Unable to reload adapters')
    expect(mocks.log).toHaveBeenCalledExactlyOnceWith({
      source: 'AgentHooksSettings',
      message: 'Agent hooks reload failed'
    })
    fireEvent.click(screen.getByRole('button', { name: 'Reload' }))
    await screen.findByRole('article')
    expectNoMutation()
  })

  it('shows a preview error without any confirmation or mutation', async () => {
    api.preview.mockRejectedValueOnce({ code: 'CONFLICT', message: 'Configuration changed' })
    render(<AgentHooksSettings />)
    fireEvent.click(await screen.findByRole('button', { name: 'Install Custom Agent' }))
    expect((await screen.findByRole('alert')).textContent).toContain('Configuration changed')
    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(mocks.log).toHaveBeenCalledExactlyOnceWith({
      source: 'AgentHooksSettings',
      message: 'Agent hooks preview failed'
    })
    expectNoMutation()
  })

  it('does not expose unexpected thrown payloads in inline errors or logs', async () => {
    api.preview.mockRejectedValueOnce(new Error('/private/path SECRET'))
    render(<AgentHooksSettings />)
    fireEvent.click(await screen.findByRole('button', { name: 'Install Custom Agent' }))
    expect((await screen.findByRole('alert')).textContent).toBe('Unable to preview changes')
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('SECRET')
    expectNoMutation()
  })

  it('discards an expired preview after confirm failure and requires a fresh preview', async () => {
    api.install.mockRejectedValueOnce({
      code: 'PLAN_EXPIRED',
      message: 'Preview expired; review again'
    })
    render(<AgentHooksSettings />)
    const dialog = await openInstall()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm install' }))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expect(screen.getByRole('alert').textContent).toContain('Preview expired')
    expect(mocks.log).toHaveBeenCalledExactlyOnceWith({
      source: 'AgentHooksSettings',
      message: 'Agent hooks confirm failed'
    })
    api.preview.mockResolvedValueOnce({ ...preview, preview_id: 'fresh-preview' })
    await openInstall()
    expect(api.preview).toHaveBeenCalledTimes(2)
    expect(api.install).toHaveBeenCalledOnce()
  })

  it('reports desktop folder errors through the log boundary', async () => {
    mocks.desktop.mockReturnValue(true)
    api.openFolder.mockRejectedValueOnce({ code: 'INTERNAL_ERROR', message: 'Folder unavailable' })
    render(<AgentHooksSettings />)
    await screen.findByRole('article')
    fireEvent.click(screen.getByRole('button', { name: 'Open folder' }))
    expect((await screen.findByRole('alert')).textContent).toContain('Folder unavailable')
    expect(mocks.log).toHaveBeenCalledExactlyOnceWith({
      source: 'AgentHooksSettings',
      message: 'Agent hooks open folder failed'
    })
    expectNoMutation()
  })
})
