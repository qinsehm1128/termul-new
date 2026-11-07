import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import { McpControlPanel } from './McpControlPanel'

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  clipboard: vi.fn(),
  loadStatus: vi.fn(),
  setBuiltInEnabled: vi.fn(),
  toast: vi.fn()
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))
vi.mock('@/lib/api', () => ({
  clipboardApi: { writeText: mocks.clipboard }
}))
vi.mock('@/hooks/use-toast', () => ({ toast: mocks.toast }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))
vi.mock('@/components/settings/McpHostAccessPanel', () => ({
  McpHostAccessPanel: () => <div data-testid="host-permissions" />
}))
vi.mock('@/components/settings/McpServersSettings', () => ({
  McpServersSettings: () => <div data-testid="upstream-servers" />
}))
vi.mock('@/stores/mcp-store', () => ({
  useMcpStore: (selector: (state: unknown) => unknown) =>
    selector({
      config: { builtIns: [{ id: 'session-memory', enabled: true }], revision: 3 },
      status: null,
      loadError: null,
      saveError: null,
      setBuiltInEnabled: mocks.setBuiltInEnabled,
      loadStatus: mocks.loadStatus
    })
}))

beforeEach(() => {
  vi.clearAllMocks()
  mocks.loadStatus.mockResolvedValue(undefined)
  mocks.clipboard.mockResolvedValue({ success: true })
  mocks.invoke.mockResolvedValue({
    success: true,
    data: {
      mode: 'direct',
      bridge: { command: '/synthetic/Se App/se-mcp', args: ['--mode', 'direct'] },
      bridgeAvailable: true,
      token: 'TOKEN_SENTINEL',
      url: 'http://localhost.invalid',
      port: 1234,
      settingsPath: '/synthetic/settings'
    }
  })
})

it.each([
  {
    format: 'MCP JSON',
    expected: JSON.stringify(
      {
        mcpServers: {
          'se-mcp': { command: '/synthetic/Se App/se-mcp', args: ['--mode', 'direct'] }
        }
      },
      null,
      2
    )
  },
  {
    format: 'Codex TOML',
    expected: [
      '[mcp_servers.se-mcp]',
      'command = "/synthetic/Se App/se-mcp"',
      'args = ["--mode", "direct"]',
      'tool_timeout_sec = 300'
    ].join('\n')
  }
])('offers only full se-mcp $format export above upstreams without mutating settings', async ({
  format,
  expected
}) => {
  const { container } = render(<McpControlPanel />)
  const full = screen.getByRole('heading', { name: 'Full Se MCP · se-mcp' })
  expect(
    full.compareDocumentPosition(screen.getByTestId('upstream-servers')) &
      Node.DOCUMENT_POSITION_FOLLOWING
  ).toBeTruthy()
  expect(screen.getByTestId('host-permissions')).toBeInTheDocument()
  expect(screen.getAllByRole('button', { name: 'Preview', exact: true })).toHaveLength(1)
  expect(screen.getAllByRole('button', { name: 'Copy full configuration' })).toHaveLength(1)
  expect(screen.queryByRole('button', { name: 'Copy config', exact: true })).not.toBeInTheDocument()
  expect(
    screen.queryByText(/se-memory|memory\.(title|description|copy|preview)/i)
  ).not.toBeInTheDocument()
  expect(mocks.invoke).not.toHaveBeenCalled()
  const card = full.closest('section')
  expect(card).not.toBeNull()
  if (!card) throw new Error('Missing full export card')
  fireEvent.click(within(card).getByRole('button', { name: format, exact: true }))
  fireEvent.click(within(card).getByRole('button', { name: 'Preview', exact: true }))
  await waitFor(() => expect(card.querySelector('pre')?.textContent).toBe(expected))
  expect(mocks.clipboard).not.toHaveBeenCalled()
  fireEvent.click(within(card).getByRole('button', { name: 'Copy full configuration' }))
  await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledExactlyOnceWith(expected))
  expect(card.querySelector('pre')?.textContent).toBe(expected)
  expect(mocks.invoke.mock.calls).toEqual([
    ['mcp_connection_info', { mode: 'direct' }],
    ['mcp_connection_info', { mode: 'direct' }]
  ])
  expect(mocks.setBuiltInEnabled).not.toHaveBeenCalled()
  expect(container.textContent).not.toContain('TOKEN_SENTINEL')
  expect(container.textContent).not.toContain('se-memory')
  expect(mocks.loadStatus).toHaveBeenCalledTimes(1)
})
