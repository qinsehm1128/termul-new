import type { PluginInfo } from '@shared/types/plugin.types'
import { act, cleanup, render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { usePluginsStore } from '@/stores/plugins-store'
import { PluginViewPage } from '../PluginViewPage'
import { PluginViewPanel } from '../PluginViewPanel'
import { webSession } from './test-fixtures'

const web = vi.hoisted(() => vi.fn())
const getView = vi.hoisted(() => vi.fn())
vi.mock('./PluginWebView', () => ({
  PluginWebView: (props: object) => {
    web(props)
    return <div data-testid="web-renderer" />
  }
}))
vi.mock('../plugin-api', () => ({ pluginApi: { getView } }))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))
const initial = usePluginsStore.getState()
const plugin: PluginInfo = {
  id: webSession.pluginId,
  desiredEnabled: true,
  state: 'active',
  generation: webSession.generation,
  configRevision: 1,
  sha256: 'a'.repeat(64),
  lastError: null,
  manifest: {
    schemaVersion: 1,
    id: webSession.pluginId,
    name: 'Web Demo',
    version: '1.0.0',
    abi: { min: 1, max: 1 },
    services: { provides: [], requires: [] },
    contributes: {
      commands: [],
      views: [
        { id: 'web', title: 'Dashboard', kind: 'web', entry: 'ui/index.html', allow: ['commands'] }
      ],
      configuration: { title: '', properties: {} }
    }
  }
}
beforeEach(() => {
  vi.clearAllMocks()
  usePluginsStore.setState({ plugins: [plugin], connected: true, loaded: true, supported: true })
})
afterEach(() => {
  cleanup()
  usePluginsStore.setState(initial, true)
})

describe('web contribution routing', () => {
  it('selects the web renderer in panels, never opens a UiTree session, and follows authority changes', () => {
    render(<PluginViewPanel pluginId={plugin.id} viewId="web" />)
    expect(screen.getByTestId('web-renderer')).toBeInTheDocument()
    expect(web).toHaveBeenLastCalledWith(
      expect.objectContaining({
        pluginId: plugin.id,
        title: 'Dashboard',
        generation: plugin.generation,
        available: true
      })
    )
    act(() => usePluginsStore.setState({ connected: false }))
    expect(web).toHaveBeenLastCalledWith(expect.objectContaining({ available: false }))
    act(() =>
      usePluginsStore.setState({
        plugins: [{ ...plugin, generation: 'new-generation', state: 'failed' }]
      })
    )
    expect(web).toHaveBeenLastCalledWith(
      expect.objectContaining({ generation: 'new-generation', crashed: true, available: false })
    )
    expect(getView).not.toHaveBeenCalled()
  })

  it('uses full-height, unconstrained sidebar route layout for a web contribution', () => {
    render(
      <MemoryRouter initialEntries={[`/plugins/${plugin.id}/web`]}>
        <Routes>
          <Route path="/plugins/:pluginId/:viewId" element={<PluginViewPage />} />
        </Routes>
      </MemoryRouter>
    )
    const container = screen.getByTestId('web-renderer').parentElement
    expect(container).toHaveClass('h-full', 'min-h-0', 'w-full', 'flex-1')
    expect(container).not.toHaveClass('max-w-6xl')
    expect(getView).not.toHaveBeenCalled()
  })
})
