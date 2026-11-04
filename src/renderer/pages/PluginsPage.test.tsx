import type { PluginInfo } from '@shared/types/plugin.types'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
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
const openTab = (tab: string) =>
  fireEvent.mouseDown(screen.getByRole('tab', { name: `manager.tabs.${tab}` }), {
    button: 0,
    ctrlKey: false
  })

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
    expect(screen.getAllByText('state.active')).toHaveLength(2)
    expect(screen.getByText('manager.version 2.0.0')).toBeInTheDocument()
    openTab('settings')
    expect(screen.getByText(`Settings: ${plugin.id}`)).toBeInTheDocument()
    openTab('contributions')
    expect(screen.getByRole('link', { name: 'Demo panel' })).toHaveAttribute(
      'href',
      `/plugins/${plugin.id}/views/main`
    )
  })

  it('keeps sidebar view card links alongside panel view links', () => {
    usePluginsStore.setState({
      plugins: [
        {
          ...plugin,
          manifest: {
            ...plugin.manifest,
            contributes: {
              ...plugin.manifest.contributes,
              views: [
                ...plugin.manifest.contributes.views,
                {
                  id: 'sidebar.counter',
                  title: 'Sidebar counter',
                  location: 'sidebar',
                  icon: 'calculator'
                }
              ]
            }
          }
        }
      ]
    })
    show()
    openTab('contributions')
    expect(screen.getByRole('link', { name: 'Demo panel' })).toHaveAttribute(
      'href',
      `/plugins/${plugin.id}/views/main`
    )
    expect(screen.getByRole('link', { name: 'Sidebar counter' })).toHaveAttribute(
      'href',
      `/plugins/${plugin.id}/views/sidebar.counter`
    )
  })

  it('filters a single native library and requires trust confirmation before install', async () => {
    show()
    fireEvent.click(screen.getByRole('button', { name: 'manager.install' }))
    await screen.findByRole('alertdialog')
    expect(tauriDialogApi.selectFile).toHaveBeenCalledWith({
      filters: [{ name: 'manager.fileType', extensions: ['dylib', 'so', 'dll'] }]
    })
    expect(
      within(screen.getByRole('alertdialog')).getByText('manager.trustWarning')
    ).toBeInTheDocument()
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

  it('disables active plugins without offering retry', () => {
    show()
    fireEvent.click(screen.getByRole('switch', { name: 'manager.disable Demo plugin' }))
    expect(actions.setEnabled).toHaveBeenCalledExactlyOnceWith(plugin.id, false)
    expect(screen.queryByRole('button', { name: 'manager.retry' })).not.toBeInTheDocument()
  })

  it('offers separate Disable and Retry actions for quarantined enabled plugins', () => {
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
    expect(screen.getAllByText('state.quarantined')).toHaveLength(2)
    expect(screen.getByText('manager.quarantineWarning')).toBeInTheDocument()
    expect(screen.getByText(/Previous process crashed/)).toBeInTheDocument()
    expect(
      screen.queryByRole('switch', { name: 'manager.enable Demo plugin' })
    ).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'manager.retry' })).toBeEnabled()
    fireEvent.click(screen.getByRole('switch', { name: 'manager.disable Demo plugin' }))
    expect(actions.setEnabled).toHaveBeenCalledExactlyOnceWith(plugin.id, false)
    fireEvent.click(screen.getByRole('button', { name: 'manager.retry' }))
    expect(actions.setEnabled).toHaveBeenCalledTimes(2)
    expect(actions.setEnabled).toHaveBeenLastCalledWith(plugin.id, true)
    expect(screen.getByRole('button', { name: 'manager.reload' })).toBeDisabled()
    expect(screen.queryByRole('link', { name: 'Demo panel' })).not.toBeInTheDocument()
  })

  it.each([
    'quarantined',
    'failed'
  ] as const)('uses only desiredEnabled for the %s toggle and explicitly retries with true', (state) => {
    usePluginsStore.setState({ plugins: [{ ...plugin, state, desiredEnabled: false }] })
    show()
    expect(
      screen.queryByRole('switch', { name: 'manager.disable Demo plugin' })
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('switch', { name: 'manager.enable Demo plugin' }))
    expect(actions.setEnabled).toHaveBeenCalledExactlyOnceWith(plugin.id, true)
    fireEvent.click(screen.getByRole('button', { name: 'manager.retry' }))
    expect(actions.setEnabled).toHaveBeenCalledTimes(2)
    expect(actions.setEnabled).toHaveBeenLastCalledWith(plugin.id, true)
  })

  it('can disable or explicitly retry a failed enabled plugin', () => {
    usePluginsStore.setState({ plugins: [{ ...plugin, state: 'failed' }] })
    show()
    fireEvent.click(screen.getByRole('switch', { name: 'manager.disable Demo plugin' }))
    expect(actions.setEnabled).toHaveBeenCalledExactlyOnceWith(plugin.id, false)
    fireEvent.click(screen.getByRole('button', { name: 'manager.retry' }))
    expect(actions.setEnabled).toHaveBeenCalledTimes(2)
    expect(actions.setEnabled).toHaveBeenLastCalledWith(plugin.id, true)
  })

  it.each([
    { busy: 'setEnabled', loading: false, connected: true },
    { busy: null, loading: true, connected: true },
    { busy: null, loading: false, connected: false }
  ])('disables explicit retry while unavailable: %j', (availability) => {
    usePluginsStore.setState({
      ...availability,
      plugins: [{ ...plugin, state: 'quarantined' }]
    })
    show()
    const retry = screen.getByRole('button', { name: 'manager.retry' })
    expect(retry).toBeDisabled()
    fireEvent.click(retry)
    expect(actions.setEnabled).not.toHaveBeenCalled()
  })

  it.each([
    'pending',
    'activating',
    'reloading'
  ] as const)('disables lifecycle buttons while %s', (state) => {
    usePluginsStore.setState({ plugins: [{ ...plugin, state }] })
    show()
    expect(screen.getByRole('switch')).toBeDisabled()
    for (const key of ['reload', 'upgrade', 'uninstall'])
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
    expect(screen.getByRole('switch', { name: 'manager.disable Demo plugin' })).toBeDisabled()
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

  it('selects another plugin and reconciles selection after removal', () => {
    const other = {
      ...plugin,
      id: 'org.termul.other',
      manifest: { ...plugin.manifest, name: 'Other plugin' }
    }
    usePluginsStore.setState({ plugins: [plugin, other] })
    const { rerender } = show()
    expect(screen.getByRole('button', { name: 'Demo plugin' })).toHaveAttribute(
      'aria-pressed',
      'true'
    )
    fireEvent.click(screen.getByRole('button', { name: 'Other plugin' }))
    expect(screen.getByRole('heading', { name: 'Other plugin' })).toBeInTheDocument()
    act(() => usePluginsStore.setState({ plugins: [plugin] }))
    rerender(
      <MemoryRouter>
        <PluginsPage />
      </MemoryRouter>
    )
    expect(screen.getByRole('heading', { name: 'Demo plugin' })).toBeInTheDocument()
  })

  it('filters by name, id and status with no-results recovery', () => {
    const disabledPlugin = {
      ...plugin,
      id: 'org.termul.other',
      state: 'disabled' as const,
      desiredEnabled: false,
      manifest: { ...plugin.manifest, name: 'Other plugin' }
    }
    const failedPlugin = {
      ...plugin,
      id: 'org.termul.failed',
      state: 'failed' as const,
      manifest: { ...plugin.manifest, name: 'Failed plugin' }
    }
    usePluginsStore.setState({ plugins: [plugin, disabledPlugin, failedPlugin] })
    show()
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'OTHER' } })
    expect(screen.getByRole('heading', { name: 'Other plugin' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Demo plugin' })).not.toBeInTheDocument()
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'org.termul.demo' } })
    expect(screen.getByRole('heading', { name: 'Demo plugin' })).toBeInTheDocument()
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'disabled' } })
    expect(screen.getByText('manager.noResults')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'manager.clearFilters' }))
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'disabled' } })
    expect(screen.getByRole('heading', { name: 'Other plugin' })).toBeInTheDocument()
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'error' } })
    expect(screen.getByRole('heading', { name: 'Failed plugin' })).toBeInTheDocument()
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'running' } })
    expect(screen.getByRole('heading', { name: 'Demo plugin' })).toBeInTheDocument()
  })

  it('switches detail tabs and gates settings when disconnected', () => {
    usePluginsStore.setState({ connected: false })
    show()
    expect(screen.getByRole('tab', { name: 'manager.tabs.overview' })).toHaveAttribute(
      'aria-selected',
      'true'
    )
    openTab('settings')
    expect(screen.getByText('manager.settingsUnavailable')).toBeInTheDocument()
    expect(screen.queryByText(`Settings: ${plugin.id}`)).not.toBeInTheDocument()
    openTab('services')
    expect(screen.getByRole('tab', { name: 'manager.tabs.services' })).toHaveAttribute(
      'aria-selected',
      'true'
    )
    openTab('overview')
    expect(screen.getByText(plugin.sha256)).toBeInTheDocument()
  })

  it('retries the disconnected Plugin Core without enabling lifecycle actions', () => {
    usePluginsStore.setState({ connected: false })
    show()
    expect(screen.getByRole('status')).toHaveTextContent('state.disconnected')
    expect(screen.getByRole('switch')).toBeDisabled()
    fireEvent.click(screen.getByRole('button', { name: 'manager.reconnect' }))
    expect(actions.load).toHaveBeenCalledExactlyOnceWith()
  })

  it('shows an install-first empty state with the same trust confirmation', async () => {
    usePluginsStore.setState({ plugins: [] })
    show()
    expect(screen.getByText('manager.emptyDescription')).toBeInTheDocument()
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'manager.installFirst' }))
    await screen.findByRole('alertdialog')
    expect(actions.install).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'manager.confirm' }))
    expect(actions.install).toHaveBeenCalledExactlyOnceWith('/tmp/plugin.dylib')
  })

  it('contains identical complete namespace keys in both locales', () => {
    const keys = (value: object, prefix = ''): string[] =>
      Object.entries(value).flatMap(([key, child]) =>
        typeof child === 'object' ? keys(child, `${prefix}${key}.`) : `${prefix}${key}`
      )
    expect(keys(en).sort()).toEqual(keys(zh).sort())
    expect(en.manager.retry).toBe('Retry')
    expect(zh.manager.retry).toBe('重试')
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
