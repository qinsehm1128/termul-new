import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  McpClient,
  McpGatewayConnection,
  McpGatewayMode,
  McpGatewayView
} from '@/lib/mcp-gateway-api'
import { useMcpGatewayStore } from '@/stores/mcp-gateway-store'
import { McpGatewayPanel } from './McpGatewayPanel'

const api = vi.hoisted(() => ({
  view: vi.fn(),
  start: vi.fn(),
  restart: vi.fn(),
  setPort: vi.fn(),
  connection: vi.fn(),
  clients: vi.fn(),
  syncClient: vi.fn(),
  unsyncClient: vi.fn()
}))
const persistence = vi.hoisted(() => ({ read: vi.fn(), write: vi.fn() }))

vi.mock('sonner', () => ({ toast: { error: vi.fn(), success: vi.fn() } }))
vi.mock('@/lib/api', () => ({
  persistenceApi: persistence,
  clipboardApi: { writeText: vi.fn(async () => ({ success: true })) }
}))
vi.mock('@/lib/mcp-gateway-api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/mcp-gateway-api')>()),
  mcpGatewayApi: api
}))

const ok = <T,>(data: T) => ({ success: true as const, data })

const running: McpGatewayView = {
  state: 'running',
  port: 3290,
  settingsPath: '/home/u/.se-manager/mcp-gateway.json',
  status: {
    version: '0.13.1',
    pid: 42,
    port: 3290,
    startedAt: 1,
    configRevision: 4,
    configError: null,
    upstreams: [
      { id: 'a', name: 'context7', state: 'connected' },
      { id: 'b', name: 'broken', state: 'failed', error: 'could not start' }
    ],
    builtIns: ['session-memory']
  }
}

function connection(mode: McpGatewayMode): McpGatewayConnection {
  return {
    mode,
    bridge: {
      command: '/Apps/se-mcp',
      args: mode === 'grouped' ? [] : ['--mode', mode]
    },
    bridgeAvailable: true,
    url: `http://127.0.0.1:3290/mcp${mode === 'entry' ? '/entry' : ''}`,
    token: 'se-mcp-token',
    port: 3290,
    settingsPath: running.settingsPath
  }
}

const clients: McpClient[] = [
  {
    id: 'claude-code',
    name: 'Claude Code',
    configPath: '/home/u/.claude.json',
    installed: true,
    synced: false,
    upToDate: false
  },
  {
    id: 'codex',
    name: 'Codex',
    configPath: '/home/u/.codex/config.toml',
    installed: true,
    synced: true,
    upToDate: false
  },
  {
    id: 'windsurf',
    name: 'Windsurf',
    configPath: '/home/u/.codeium/windsurf/mcp_config.json',
    installed: false,
    synced: false,
    upToDate: false
  }
]

function row(name: string): HTMLElement {
  const item = screen.getByText(name).closest('li')
  if (!item) throw new Error(`no row for ${name}`)
  return item
}

describe('McpGatewayPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useMcpGatewayStore.setState({
      mode: 'grouped',
      view: null,
      connection: null,
      clients: [],
      busy: null,
      error: null
    })
    persistence.read.mockResolvedValue({ success: false, code: 'KEY_NOT_FOUND' })
    persistence.write.mockResolvedValue({ success: true })
    api.view.mockResolvedValue(ok(running))
    api.connection.mockImplementation(async (mode: McpGatewayMode) => ok(connection(mode)))
    api.clients.mockResolvedValue(ok(clients))
    api.syncClient.mockResolvedValue(ok({ configPath: '/home/u/.claude.json' }))
    api.unsyncClient.mockResolvedValue(ok({ configPath: '/home/u/.codex/config.toml' }))
    api.setPort.mockResolvedValue(ok(running))
  })

  it('shows the running gateway and each client with its state', async () => {
    render(<McpGatewayPanel />)
    expect(await screen.findByText('Running · v0.13.1')).toBeTruthy()
    expect(screen.getByText('1 connected · 1 failed · 0 starting')).toBeTruthy()
    expect(within(row('Claude Code')).getByText('Not connected')).toBeTruthy()
    expect(within(row('Codex')).getByText('Needs update')).toBeTruthy()
    const windsurf = row('Windsurf')
    expect(within(windsurf).getByText('Not installed')).toBeTruthy()
    expect(
      (within(windsurf).getByRole('button', { name: 'Connect' }) as HTMLButtonElement).disabled
    ).toBe(true)
  })

  it('connects a client in the selected mode and re-reads the clients', async () => {
    render(<McpGatewayPanel />)
    await screen.findByText('Claude Code')
    fireEvent.click(screen.getByRole('button', { name: /^Entry/ }))
    await waitFor(() => expect(api.clients).toHaveBeenLastCalledWith('entry'))
    expect(persistence.write).toHaveBeenCalledWith('mcp/gateway-mode', 'entry')

    fireEvent.click(within(row('Claude Code')).getByRole('button', { name: 'Connect' }))
    await waitFor(() => expect(api.syncClient).toHaveBeenCalledWith('claude-code', 'entry'))
    await waitFor(() => expect(api.clients).toHaveBeenCalledTimes(3))
  })

  it('updates a stale entry and removes one', async () => {
    render(<McpGatewayPanel />)
    await screen.findByText('Codex')
    fireEvent.click(within(row('Codex')).getByRole('button', { name: 'Update' }))
    await waitFor(() => expect(api.syncClient).toHaveBeenCalledWith('codex', 'grouped'))
    fireEvent.click(within(row('Codex')).getByRole('button', { name: 'Remove' }))
    await waitFor(() => expect(api.unsyncClient).toHaveBeenCalledWith('codex'))
  })

  it('moves the gateway to a new port only when it changed and is valid', async () => {
    render(<McpGatewayPanel />)
    const input = (await screen.findByLabelText('Port')) as HTMLInputElement
    await waitFor(() => expect(input.value).toBe('3290'))
    const apply = screen.getByRole('button', { name: 'Apply' }) as HTMLButtonElement
    expect(apply.disabled).toBe(true)
    fireEvent.change(input, { target: { value: '80' } })
    expect(apply.disabled).toBe(true)
    fireEvent.change(input, { target: { value: '4555' } })
    fireEvent.click(apply)
    await waitFor(() => expect(api.setPort).toHaveBeenCalledWith(4555))
  })

  it('offers to start a stopped gateway', async () => {
    api.view.mockResolvedValue(ok({ ...running, state: 'stopped', status: undefined }))
    api.start.mockResolvedValue(ok(running))
    render(<McpGatewayPanel />)
    fireEvent.click(await screen.findByRole('button', { name: 'Start' }))
    await waitFor(() => expect(api.start).toHaveBeenCalledTimes(1))
  })
})
