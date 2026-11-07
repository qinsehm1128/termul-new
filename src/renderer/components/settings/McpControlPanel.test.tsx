import { MCP_GATEWAY_CLIENT_KEYS, MCP_GATEWAY_SOURCES } from '@shared/types/mcp-gateway.types'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import en from '@/locales/en/mcp.json'
import { McpControlPanel } from './McpControlPanel'

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  clipboard: vi.fn(),
  loadStatus: vi.fn(),
  setBuiltInEnabled: vi.fn(),
  toast: vi.fn(),
  desktop: true
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('@/lib/api', () => ({ clipboardApi: { writeText: mocks.clipboard } }))
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
  mocks.desktop = true
  mocks.loadStatus.mockResolvedValue(undefined)
  mocks.clipboard.mockResolvedValue({ success: true })
  mocks.invoke.mockImplementation(async (_command, { source }) => ({
    success: true,
    data: {
      source,
      mode: 'direct',
      httpReady: false,
      bridge: {
        command: '/synthetic/Se App/se-mcp',
        args: ['--source', source, '--mode', 'direct']
      },
      bridgeAvailable: true,
      token: 'TOKEN_SENTINEL',
      url: 'http://localhost.invalid',
      port: source === 'internal' ? 3290 : 3390,
      settingsPath: `/synthetic/${source}`
    }
  }))
})

it.each([
  'MCP JSON',
  'Codex TOML'
])('offers both source-specific %s exports above upstreams without mutating settings', async (format) => {
  const { container } = render(<McpControlPanel />)
  expect(screen.getByTestId('host-permissions')).toBeInTheDocument()
  expect(screen.getAllByRole('button', { name: en.export.preview, exact: true })).toHaveLength(2)
  expect(screen.getAllByRole('button', { name: en.export.copy })).toHaveLength(2)
  expect(mocks.invoke).not.toHaveBeenCalled()
  for (const source of MCP_GATEWAY_SOURCES) {
    const heading = screen.getByRole('heading', { name: en.gateway.sources[source].title })
    expect(
      heading.compareDocumentPosition(screen.getByTestId('upstream-servers')) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    const card = heading.closest('section')
    if (!card) throw new Error('Missing source export card')
    const key = MCP_GATEWAY_CLIENT_KEYS[source]
    const bridge = {
      command: '/synthetic/Se App/se-mcp',
      args: ['--source', source, '--mode', 'direct']
    }
    const expected =
      format === 'MCP JSON'
        ? JSON.stringify({ mcpServers: { [key]: bridge } }, null, 2)
        : [
            `[mcp_servers.${key}]`,
            'command = "/synthetic/Se App/se-mcp"',
            `args = ["--source", "${source}", "--mode", "direct"]`,
            'tool_timeout_sec = 300'
          ].join('\n')
    fireEvent.click(within(card).getByRole('button', { name: format, exact: true }))
    fireEvent.click(within(card).getByRole('button', { name: en.export.preview, exact: true }))
    await waitFor(() => expect(card.querySelector('pre')?.textContent).toBe(expected))
    fireEvent.click(within(card).getByRole('button', { name: en.export.copy }))
    await waitFor(() => expect(mocks.clipboard).toHaveBeenLastCalledWith(expected))
    expect(card.querySelector('pre')?.textContent).toBe(expected)
  }
  expect(mocks.invoke.mock.calls).toEqual(
    MCP_GATEWAY_SOURCES.flatMap((source) => [
      ['mcp_connection_info', { source, mode: 'direct' }],
      ['mcp_connection_info', { source, mode: 'direct' }]
    ])
  )
  expect(mocks.setBuiltInEnabled).not.toHaveBeenCalled()
  expect(container.textContent).not.toContain('TOKEN_SENTINEL')
  expect(container.textContent).not.toContain('se-memory')
  expect(mocks.loadStatus).toHaveBeenCalledTimes(1)
})

it('keeps both local exports unavailable on Web without local invokes', () => {
  mocks.desktop = false
  render(<McpControlPanel />)
  expect(screen.getAllByText(en.export.desktopOnly)).toHaveLength(2)
  for (const button of screen.getAllByRole('button', { name: en.export.copy })) {
    expect(button).toBeDisabled()
  }
  expect(mocks.invoke).not.toHaveBeenCalled()
  expect(mocks.clipboard).not.toHaveBeenCalled()
})
