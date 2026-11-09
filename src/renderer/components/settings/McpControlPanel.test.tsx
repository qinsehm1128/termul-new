import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import en from '@/locales/en/mcp.json'
import { McpCapabilitiesPanel, McpUpstreamsPanel } from './McpControlPanel'

const mocks = vi.hoisted(() => ({
  loadStatus: vi.fn(),
  setBuiltInEnabled: vi.fn(),
  saveError: null as string | null
}))
vi.mock('@/components/settings/McpHostAccessPanel', () => ({
  McpHostAccessPanel: () => <div data-testid="host-permissions" />
}))
vi.mock('@/components/settings/McpServersSettings', () => ({
  McpServersSettings: () => <div data-testid="upstream-servers" />
}))
vi.mock('@/stores/mcp-store', () => ({
  useMcpStore: (selector: (state: unknown) => unknown) =>
    selector({
      config: {
        builtIns: [
          { id: 'ssh', enabled: false },
          { id: 'project-scope', enabled: true },
          { id: 'session-memory', enabled: true }
        ],
        revision: 3
      },
      status: null,
      loadError: null,
      saveError: mocks.saveError,
      setBuiltInEnabled: mocks.setBuiltInEnabled,
      loadStatus: mocks.loadStatus
    })
}))

beforeEach(() => {
  vi.clearAllMocks()
  mocks.saveError = null
  mocks.loadStatus.mockResolvedValue(undefined)
})

it('keeps the external servers tab to the upstream registry only', () => {
  render(<McpUpstreamsPanel />)
  expect(screen.getByTestId('upstream-servers')).toBeInTheDocument()
  expect(screen.queryByTestId('host-permissions')).toBeNull()
  expect(screen.queryByRole('button', { name: en.export.copy })).toBeNull()
  expect(mocks.loadStatus).toHaveBeenCalledTimes(1)
})

it('puts built-in switches in order above host access, hiding project scope', () => {
  render(<McpCapabilitiesPanel />)
  const switches = screen.getAllByRole('switch')
  expect(switches.map((item) => item.getAttribute('aria-label'))).toEqual([
    en.common.disable.replace('{{name}}', en.page.builtIn['session-memory'].name),
    en.common.enable.replace('{{name}}', en.page.builtIn.ssh.name)
  ])
  const heading = screen.getByRole('heading', { name: en.page.builtInsTitle })
  expect(
    heading.compareDocumentPosition(screen.getByTestId('host-permissions')) &
      Node.DOCUMENT_POSITION_FOLLOWING
  ).toBeTruthy()
  fireEvent.click(switches[1])
  expect(mocks.setBuiltInEnabled).toHaveBeenCalledWith('ssh', true)
  expect(screen.queryByTestId('upstream-servers')).toBeNull()
})

it('shows store save errors on both tabs', () => {
  mocks.saveError = 'save failed'
  const { unmount } = render(<McpUpstreamsPanel />)
  expect(screen.getByRole('alert')).toHaveTextContent('save failed')
  unmount()
  render(<McpCapabilitiesPanel />)
  expect(screen.getByRole('alert')).toHaveTextContent('save failed')
})
