import type { ConfigSnapshot, PluginInfo } from '@shared/types/plugin.types'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { expect, it, vi } from 'vitest'
import { pluginApi } from '../plugin-api'
import { PluginDetails } from './PluginDetails'

vi.mock('../plugin-api', () => ({
  pluginApi: {
    getConfig: vi.fn(),
    setConfig: vi.fn(),
    listProcesses: vi.fn(),
    getProcessLogs: vi.fn(),
    controlProcess: vi.fn()
  }
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))

const plugin: PluginInfo = {
  id: 'org.termul.demo',
  sha256: 'a'.repeat(64),
  state: 'active',
  desiredEnabled: true,
  generation: 'generation',
  lastError: null,
  manifest: {
    id: 'org.termul.demo',
    name: 'Demo plugin',
    version: '1.0.0',
    abi: { min: 1, max: 1 },
    contributes: {
      commands: [],
      views: [],
      eventSubscriptions: [],
      menus: [],
      keybindings: [],
      statusItems: [],
      configuration: { title: 'Demo settings', properties: {} }
    },
    services: { provides: [], requires: [] }
  }
}

it('loads settings lazily and preserves unsaved edits across detail tabs', async () => {
  const config: ConfigSnapshot = {
    pluginId: plugin.id,
    revision: 1,
    schema: {
      title: 'Demo settings',
      properties: {
        greeting: {
          type: 'string',
          title: 'Greeting',
          description: 'Greeting text',
          default: 'Hello'
        }
      }
    },
    values: { greeting: 'Hello' },
    overrides: {}
  }
  vi.mocked(pluginApi.getConfig).mockResolvedValue({ success: true, data: config })
  render(
    <MemoryRouter>
      <PluginDetails
        plugin={plugin}
        plugins={[plugin]}
        connected
        disabled={false}
        onRetry={vi.fn()}
        onReload={vi.fn()}
        onUpgrade={vi.fn()}
        onUninstall={vi.fn()}
      />
    </MemoryRouter>
  )
  const openTab = (tab: string) =>
    fireEvent.mouseDown(screen.getByRole('tab', { name: `manager.tabs.${tab}` }), {
      button: 0,
      ctrlKey: false
    })
  expect(pluginApi.getConfig).not.toHaveBeenCalled()
  openTab('settings')
  const greeting = await screen.findByRole('textbox', { name: 'Greeting' })
  fireEvent.change(greeting, { target: { value: 'Unsaved change' } })
  openTab('overview')
  expect(screen.queryByRole('textbox', { name: 'Greeting' })).not.toBeInTheDocument()
  expect(greeting).toBeInTheDocument()
  expect(greeting).not.toBeVisible()
  openTab('services')
  openTab('settings')
  expect(screen.getByRole('textbox', { name: 'Greeting' })).toHaveValue('Unsaved change')
  expect(pluginApi.getConfig).toHaveBeenCalledTimes(1)
  expect(pluginApi.setConfig).not.toHaveBeenCalled()
})

it('polls processes only while the Processes tab is open', async () => {
  cleanup()
  vi.useFakeTimers()
  vi.mocked(pluginApi.listProcesses).mockResolvedValue({ success: true, data: { processes: [] } })
  vi.mocked(pluginApi.getProcessLogs).mockResolvedValue({
    success: true,
    data: { lines: [], truncated: false }
  })
  const processPlugin: PluginInfo = {
    ...plugin,
    manifest: {
      ...plugin.manifest,
      contributes: {
        ...plugin.manifest.contributes,
        processes: [
          { id: 'worker', command: 'worker', args: [], env: {}, restart: 'never', autostart: false }
        ]
      }
    }
  }
  try {
    render(
      <MemoryRouter>
        <PluginDetails
          plugin={processPlugin}
          plugins={[processPlugin]}
          connected
          disabled={false}
          onRetry={vi.fn()}
          onReload={vi.fn()}
          onUpgrade={vi.fn()}
          onUninstall={vi.fn()}
        />
      </MemoryRouter>
    )
    expect(pluginApi.listProcesses).not.toHaveBeenCalled()
    fireEvent.mouseDown(screen.getByRole('tab', { name: 'manager.tabs.processes' }), {
      button: 0,
      ctrlKey: false
    })
    await act(async () => {
      await Promise.resolve()
    })
    expect(pluginApi.listProcesses).toHaveBeenCalledTimes(1)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(pluginApi.listProcesses).toHaveBeenCalledTimes(2)
    fireEvent.mouseDown(screen.getByRole('tab', { name: 'manager.tabs.overview' }), {
      button: 0,
      ctrlKey: false
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6000)
    })
    expect(pluginApi.listProcesses).toHaveBeenCalledTimes(2)
    expect(screen.queryByText('processes.refreshHint')).not.toBeInTheDocument()
  } finally {
    cleanup()
    vi.useRealTimers()
  }
})
