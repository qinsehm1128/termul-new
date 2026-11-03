import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { PluginViewPage } from './PluginViewPage'

const control = vi.hoisted(() => ({ crash: false }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn().mockResolvedValue(undefined) }))

vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))
vi.mock('./PluginViewPanel', () => ({
  PluginViewPanel: ({ pluginId, viewId }: { pluginId: string; viewId: string }) => {
    if (control.crash) throw new Error('private render error')
    return (
      <div data-testid="plugin-panel">
        {pluginId}:{viewId}
      </div>
    )
  }
}))
afterEach(cleanup)

describe('PluginViewPage', () => {
  it('contains rendering errors and allows recovery without exposing raw errors', () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    const preventExpectedError = (event: ErrorEvent): void => {
      event.preventDefault()
    }
    window.addEventListener('error', preventExpectedError)
    control.crash = true
    try {
      render(
        <MemoryRouter initialEntries={['/plugins/org.termul.example/views/counter']}>
          <Routes>
            <Route path="/plugins/:pluginId/views/:viewId" element={<PluginViewPage />} />
          </Routes>
        </MemoryRouter>
      )
      expect(screen.getByRole('alert')).toHaveTextContent('errors.generic')
      expect(screen.queryByText('private render error')).not.toBeInTheDocument()
      control.crash = false
      fireEvent.click(screen.getByRole('button', { name: 'view.retry' }))
      expect(screen.getByTestId('plugin-panel')).toBeInTheDocument()
    } finally {
      window.removeEventListener('error', preventExpectedError)
      control.crash = false
      consoleError.mockRestore()
    }
  })
  it('passes the /plugins/:pluginId/views/:viewId route parameters to the panel', () => {
    render(
      <MemoryRouter initialEntries={['/plugins/org.termul.example/views/counter']}>
        <Routes>
          <Route path="/plugins/:pluginId/views/:viewId" element={<PluginViewPage />} />
        </Routes>
      </MemoryRouter>
    )
    expect(screen.getByTestId('plugin-panel')).toHaveTextContent('org.termul.example:counter')
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('renders an unavailable fallback instead of mounting with missing parameters', () => {
    render(
      <MemoryRouter>
        <PluginViewPage />
      </MemoryRouter>
    )
    expect(screen.getByRole('alert')).toHaveTextContent('view.unavailable')
    expect(screen.queryByTestId('plugin-panel')).not.toBeInTheDocument()
  })
})
