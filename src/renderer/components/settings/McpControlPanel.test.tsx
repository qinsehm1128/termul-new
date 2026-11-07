import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import { McpControlPanel } from './McpControlPanel'

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  clipboard: vi.fn(),
  memory: vi.fn(),
  loadStatus: vi.fn(),
  setBuiltInEnabled: vi.fn(),
  toast: vi.fn()
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))
vi.mock('@/lib/api', () => ({
  clipboardApi: { writeText: mocks.clipboard },
  memoryIndexApi: { universalMcpInvocation: mocks.memory }
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

it('prioritizes full export above upstreams and optional memory-only compatibility, without mutating settings', async () => {
  const { container } = render(<McpControlPanel />)
  const full = screen.getByRole('heading', { name: 'Full Se MCP · se-mcp' })
  const memory = screen.getByText('Memory only · se-memory (compatibility)')
  expect(
    full.compareDocumentPosition(screen.getByTestId('upstream-servers')) &
      Node.DOCUMENT_POSITION_FOLLOWING
  ).toBeTruthy()
  expect(full.compareDocumentPosition(memory) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  expect(mocks.invoke).not.toHaveBeenCalled()
  expect(mocks.memory).not.toHaveBeenCalled()
  expect(
    screen.getByText(/It does not expose workspace, SSH, or upstream tools/)
  ).toBeInTheDocument()
  const card = full.closest('section')
  expect(card).not.toBeNull()
  if (!card) throw new Error('Missing full export card')
  fireEvent.click(within(card).getByRole('button', { name: 'Copy full configuration' }))
  await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(1))
  expect(mocks.invoke.mock.calls).toEqual([['mcp_connection_info', { mode: 'direct' }]])
  expect(mocks.setBuiltInEnabled).not.toHaveBeenCalled()
  expect(mocks.memory).not.toHaveBeenCalled()
  expect(container.textContent).not.toContain('TOKEN_SENTINEL')
  expect(mocks.loadStatus).toHaveBeenCalledTimes(1)
})
