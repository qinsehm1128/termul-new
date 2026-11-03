import type { PluginInfo } from '@shared/types/plugin.types'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { tauriDialogApi } from '@/lib/tauri-dialog-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import en from '@/locales/en/plugins.json'
import zh from '@/locales/zh-CN/plugins.json'
import { usePluginsStore } from '@/stores/plugins-store'
import PluginsPage from './PluginsPage'

vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: vi.fn(() => true) }))
vi.mock('@/lib/tauri-dialog-api', () => ({ tauriDialogApi: { selectFile: vi.fn() } }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(async () => {}) }))
vi.mock('@/features/plugins/PluginSettingsForm', () => ({
  PluginSettingsForm: ({ pluginId }: { pluginId: string }) => <div>Settings: {pluginId}</div>
}))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))

const actions = {
  install: vi.fn(),
  setEnabled: vi.fn(),
  reload: vi.fn(),
  uninstall: vi.fn(),
  load: vi.fn()
}
const plugin: PluginInfo = {
  id: 'org.termul.demo',
  desiredEnabled: true,
  state: 'active',
  generation: 'g1',
  configRevision: 1,
  sha256: 'a'.repeat(64),
  lastError: null,
  manifest: {
    schemaVersion: 1,
    id: 'org.termul.demo',
    name: 'Demo plugin',
    version: '2.0.0',
    abi: { min: 1, max: 1 },
    services: { provides: [], requires: [] },
    contributes: {
      commands: [],
      configuration: { title: '', properties: {} },
      views: [{ id: 'main', title: 'Demo panel', location: 'panel' }]
    }
  }
}
const show = () =>
  render(
    <MemoryRouter>
      <PluginsPage />
    </MemoryRouter>
  )
beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(isTauriContext).mockReturnValue(true)
  usePluginsStore.setState({
    ...actions,
    plugins: [plugin],
    connected: true,
    busy: null,
    loading: false,
    error: null
  })
  vi.mocked(tauriDialogApi.selectFile).mockResolvedValue({
    success: true,
    data: '/tmp/plugin.dylib'
  })
})

describe('PluginsPage', () => {
  it('shows state, version, settings and dedicated panel links', () => {
    show()
    expect(screen.getByRole('heading', { name: 'Demo plugin' })).toBeInTheDocument()
    expect(screen.getByText('state.active')).toBeInTheDocument()
    expect(screen.getByText('manager.version 2.0.0')).toBeInTheDocument()
    expect(screen.getByText(`Settings: ${plugin.id}`)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Demo panel' })).toHaveAttribute(
      'href',
      `/plugins/${plugin.id}/views/main`
    )
  })

  it('filters a single native library and requires trust confirmation before install', async () => {
    show()
    fireEvent.click(screen.getByRole('button', { name: 'manager.install' }))
    await screen.findByRole('alertdialog')
    expect(tauriDialogApi.selectFile).toHaveBeenCalledWith({
      filters: [{ name: 'manager.fileType', extensions: ['dylib', 'so', 'dll'] }]
    })
    expect(screen.getByText('manager.trustWarning')).toBeInTheDocument()
    expect(actions.install).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'manager.confirm' }))
    expect(actions.install).toHaveBeenCalledWith('/tmp/plugin.dylib')
  })

  it('does nothing when file selection is cancelled or trust is declined', async () => {
    vi.mocked(tauriDialogApi.selectFile).mockResolvedValueOnce({ success: true, data: null })
    show()
    fireEvent.click(screen.getByRole('button', { name: 'manager.install' }))
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'manager.install' })).toBeEnabled()
    )
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'manager.install' }))
    await screen.findByRole('alertdialog')
    fireEvent.click(screen.getByRole('button', { name: 'manager.cancel' }))
    expect(actions.install).not.toHaveBeenCalled()
  })

  it('plain reload uses the saved path while upgrade picks and confirms a replacement', async () => {
    show()
    fireEvent.click(screen.getByRole('button', { name: 'manager.reload' }))
    expect(actions.reload).toHaveBeenLastCalledWith(plugin.id)
    expect(tauriDialogApi.selectFile).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'manager.upgrade' }))
    await screen.findByRole('alertdialog')
    expect(actions.reload).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: 'manager.confirm' }))
    expect(actions.reload).toHaveBeenLastCalledWith(plugin.id, '/tmp/plugin.dylib')
  })

  it('requires explicit confirmation that uninstall deletes saved configuration', () => {
    show()
    fireEvent.click(screen.getByRole('button', { name: 'manager.uninstall' }))
    expect(screen.getByText('manager.uninstallWarning')).toBeInTheDocument()
    expect(actions.uninstall).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'manager.confirm' }))
    expect(actions.uninstall).toHaveBeenCalledWith(plugin.id)
    expect(en.manager.uninstallWarning).toContain('configuration')
    expect(zh.manager.uninstallWarning).toContain('配置')
  })

  it('disables active plugins and explicitly re-enables quarantined plugins', () => {
    const first = show()
    fireEvent.click(screen.getByRole('button', { name: 'manager.disable' }))
    expect(actions.setEnabled).toHaveBeenCalledWith(plugin.id, false)
    first.unmount()
    usePluginsStore.setState({
      plugins: [
        {
          ...plugin,
          state: 'quarantined',
          lastError: { code: 'PLUGIN_CRASH', message: 'Previous process crashed' }
        }
      ]
    })
    show()
    expect(screen.getByText('state.quarantined')).toBeInTheDocument()
    expect(screen.getByText('manager.quarantineWarning')).toBeInTheDocument()
    expect(screen.getByText(/Previous process crashed/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'manager.enable' }))
    expect(actions.setEnabled).toHaveBeenLastCalledWith(plugin.id, true)
    expect(screen.getByRole('button', { name: 'manager.reload' })).toBeDisabled()
    expect(screen.queryByRole('link', { name: 'Demo panel' })).not.toBeInTheDocument()
  })

  it.each([
    'pending',
    'activating',
    'reloading'
  ] as const)('disables lifecycle buttons while %s', (state) => {
    usePluginsStore.setState({ plugins: [{ ...plugin, state }] })
    show()
    for (const key of ['disable', 'reload', 'upgrade', 'uninstall'])
      expect(screen.getByRole('button', { name: `manager.${key}` })).toBeDisabled()
  })

  it('disables actions while busy and shows disconnected/error recovery', () => {
    usePluginsStore.setState({
      busy: 'reload',
      connected: false,
      error: { code: 'PLUGIN_IO', message: 'internal' }
    })
    show()
    expect(screen.getByRole('button', { name: 'manager.install' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'manager.disable' })).toBeDisabled()
    expect(screen.getByText('state.disconnected')).toBeInTheDocument()
    expect(screen.getByRole('alert')).toHaveTextContent('errors.generic (PLUGIN_IO)')
    expect(screen.queryByText('internal')).not.toBeInTheDocument()
  })

  it('surfaces picker failure and permits retry', async () => {
    vi.mocked(tauriDialogApi.selectFile).mockResolvedValueOnce({
      success: false,
      error: 'bad picker'
    })
    show()
    fireEvent.click(screen.getByRole('button', { name: 'manager.install' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('errors.generic')
    expect(screen.getByRole('button', { name: 'manager.install' })).toBeEnabled()
    expect(actions.install).not.toHaveBeenCalled()
  })

  it('browser displays desktop-only fallback without APIs, dialogs or HTTP', () => {
    vi.mocked(isTauriContext).mockReturnValue(false)
    const fetch = vi.spyOn(globalThis, 'fetch')
    show()
    expect(screen.getByRole('status')).toHaveTextContent('manager.unsupported')
    expect(screen.queryByRole('button', { name: 'manager.install' })).not.toBeInTheDocument()
    expect(tauriDialogApi.selectFile).not.toHaveBeenCalled()
    for (const action of Object.values(actions)) expect(action).not.toHaveBeenCalled()
    expect(fetch).not.toHaveBeenCalled()
    fetch.mockRestore()
  })

  it('contains identical complete namespace keys in both locales', () => {
    const keys = (value: object, prefix = ''): string[] =>
      Object.entries(value).flatMap(([key, child]) =>
        typeof child === 'object' ? keys(child, `${prefix}${key}.`) : `${prefix}${key}`
      )
    expect(keys(en).sort()).toEqual(keys(zh).sort())
    for (const state of [
      'installed',
      'disabled',
      'pending',
      'activating',
      'active',
      'reloading',
      'failed',
      'quarantined',
      'disconnected'
    ])
      expect(keys(en)).toContain(`state.${state}`)
    for (const key of [
      'errors.generic',
      'settings.default',
      'settings.empty',
      'settings.reload',
      'settings.reset',
      'settings.save',
      'settings.saving',
      'settings.title',
      'settings.conflict',
      'settings.invalid',
      'view.invalid',
      'view.loading',
      'view.retry',
      'view.unavailable'
    ])
      expect(keys(en)).toContain(key)
  })
})
