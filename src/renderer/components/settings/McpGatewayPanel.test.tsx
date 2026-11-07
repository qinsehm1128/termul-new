/** @vitest-environment jsdom */
import { MCP_GATEWAY_SOURCES, type McpGatewaySource } from '@shared/types/mcp-gateway.types'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { createInstance } from 'i18next'
import { I18nextProvider } from 'react-i18next'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { McpClient, McpGatewayConnection, McpGatewayView } from '@/lib/mcp-gateway-api'
import en from '@/locales/en/mcp.json'

const mocks = vi.hoisted(() => ({
  desktop: true,
  view: vi.fn(),
  start: vi.fn(),
  restart: vi.fn(),
  setPort: vi.fn(),
  connection: vi.fn(),
  clients: vi.fn(),
  syncClient: vi.fn(),
  unsyncClient: vi.fn(),
  read: vi.fn(),
  write: vi.fn(),
  clipboard: vi.fn(),
  success: vi.fn(),
  error: vi.fn()
}))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('@/lib/api', () => ({
  persistenceApi: { read: mocks.read, write: mocks.write },
  clipboardApi: { writeText: mocks.clipboard }
}))
vi.mock('sonner', () => ({ toast: { success: mocks.success, error: mocks.error } }))
vi.mock('@/lib/mcp-gateway-api', async (original) => ({
  ...(await original<typeof import('@/lib/mcp-gateway-api')>()),
  mcpGatewayApi: {
    view: mocks.view,
    start: mocks.start,
    restart: mocks.restart,
    setPort: mocks.setPort,
    connection: mocks.connection,
    clients: mocks.clients,
    syncClient: mocks.syncClient,
    unsyncClient: mocks.unsyncClient
  }
}))

import { type McpGatewaySourceState, useMcpGatewayStore } from '@/stores/mcp-gateway-store'
import { McpGatewayPanel } from './McpGatewayPanel'

const SECRET = 'NEVER_LOG_GATEWAY_TOKEN'
function view(source: McpGatewaySource): McpGatewayView {
  return {
    source,
    state: 'running',
    port: source === 'internal' ? 3290 : 3390,
    settingsPath: `/state/${source}.json`,
    status: {
      source,
      protocol: 'se-mcp-gateway',
      protocolVersion: 1,
      pid: 42,
      port: source === 'internal' ? 3290 : 3390,
      version: '1.0',
      startedAt: 0,
      configRevision: 1,
      upstreams: [],
      builtIns: [],
      configError: null
    }
  }
}
function connection(source: McpGatewaySource, mode = 'grouped'): McpGatewayConnection {
  return {
    source,
    mode,
    httpReady: true,
    bridgeAvailable: true,
    url: `http://127.0.0.1:${source === 'internal' ? 3290 : 3390}/mcp`,
    token: SECRET,
    port: source === 'internal' ? 3290 : 3390,
    settingsPath: `/state/${source}.json`,
    bridge: { command: '/app/se-mcp', args: ['--source', source, '--mode', mode] }
  } as McpGatewayConnection
}
const client: McpClient = {
  id: 'claude-desktop',
  name: 'Claude Desktop',
  configPath: '/home/claude.json',
  installed: true,
  synced: false,
  upToDate: false
}
function patchSource(source: McpGatewaySource, patch: Partial<McpGatewaySourceState>) {
  useMcpGatewayStore.setState((state) => ({
    sources: { ...state.sources, [source]: { ...state.sources[source], ...patch } }
  }))
}
function section(source: McpGatewaySource) {
  return within(screen.getAllByRole('region')[source === 'internal' ? 0 : 1])
}
async function mount() {
  const i18n = createInstance()
  await i18n.init({
    lng: 'en',
    resources: { en: { mcp: en } },
    interpolation: { escapeValue: false }
  })
  render(
    <I18nextProvider i18n={i18n}>
      <McpGatewayPanel />
    </I18nextProvider>
  )
  if (mocks.desktop) await waitFor(() => expect(mocks.connection).toHaveBeenCalledTimes(2))
}
beforeEach(() => {
  vi.resetAllMocks()
  mocks.desktop = true
  mocks.view.mockImplementation(async (source) => ({ success: true, data: view(source) }))
  mocks.connection.mockImplementation(async (source, mode) => ({
    success: true,
    data: connection(source, mode)
  }))
  mocks.clients.mockResolvedValue({ success: true, data: [client] })
  mocks.read.mockResolvedValue({ success: true, data: null })
  mocks.write.mockResolvedValue({ success: true })
  mocks.clipboard.mockResolvedValue({ success: true })
  for (const fn of [
    mocks.start,
    mocks.restart,
    mocks.setPort,
    mocks.syncClient,
    mocks.unsyncClient
  ]) {
    fn.mockResolvedValue({ success: true, data: null })
  }
  useMcpGatewayStore.setState({
    sources: {
      internal: {
        mode: 'grouped',
        view: null,
        connection: null,
        clients: [],
        busy: null,
        error: null
      },
      aggregator: {
        mode: 'grouped',
        view: null,
        connection: null,
        clients: [],
        busy: null,
        error: null
      }
    }
  })
})
afterEach(cleanup)

describe('independent MCP source connections', () => {
  it('reads both sources on mount without starting or syncing either', async () => {
    await mount()
    expect(screen.getAllByRole('region')).toHaveLength(2)
    for (const source of MCP_GATEWAY_SOURCES) {
      expect(mocks.view).toHaveBeenCalledWith(source)
      expect(mocks.connection).toHaveBeenCalledWith(source, 'grouped')
      expect(mocks.clients).toHaveBeenCalledWith(source, 'grouped')
      expect(section(source).getByText('Claude Desktop')).toBeTruthy()
    }
    expect(mocks.start).not.toHaveBeenCalled()
    expect(mocks.restart).not.toHaveBeenCalled()
    expect(mocks.syncClient).not.toHaveBeenCalled()
  })

  it('keeps mode changes and persisted preferences separate', async () => {
    await mount()
    fireEvent.click(section('aggregator').getByRole('button', { name: /Direct/ }))
    await waitFor(() => expect(mocks.connection).toHaveBeenCalledWith('aggregator', 'direct'))
    expect(useMcpGatewayStore.getState().sources.internal.mode).toBe('grouped')
    expect(
      section('internal')
        .getByRole('button', { name: /Grouped/ })
        .getAttribute('aria-pressed')
    ).toBe('true')
    expect(mocks.write).toHaveBeenCalledWith('mcp/gateway-modes', {
      internal: 'grouped',
      aggregator: 'direct'
    })
    fireEvent.click(section('internal').getByRole('button', { name: /Entry/ }))
    await waitFor(() => expect(mocks.connection).toHaveBeenCalledWith('internal', 'entry'))
    expect(useMcpGatewayStore.getState().sources.aggregator.mode).toBe('direct')
  })

  it('loads saved source modes before requesting their connections', async () => {
    mocks.read.mockResolvedValue({
      success: true,
      data: { internal: 'entry', aggregator: 'direct' }
    })
    await mount()
    expect(mocks.connection).toHaveBeenCalledWith('internal', 'entry')
    expect(mocks.connection).toHaveBeenCalledWith('aggregator', 'direct')
    expect(mocks.read).toHaveBeenCalledTimes(1)
  })

  it('syncs both sources and never lets one busy source disable the other', async () => {
    let finish!: (result: { success: boolean }) => void
    mocks.syncClient.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        })
    )
    await mount()
    fireEvent.click(section('internal').getByRole('button', { name: en.gateway.clients.connect }))
    await waitFor(() =>
      expect(useMcpGatewayStore.getState().sources.internal.busy).toBe('sync:claude-desktop')
    )
    expect(
      section('internal')
        .getByRole('button', { name: /Direct/ })
        .hasAttribute('disabled')
    ).toBe(true)
    expect(
      section('aggregator')
        .getByRole('button', { name: /Direct/ })
        .hasAttribute('disabled')
    ).toBe(false)
    fireEvent.click(section('aggregator').getByRole('button', { name: en.gateway.clients.connect }))
    await waitFor(() =>
      expect(mocks.syncClient).toHaveBeenCalledWith('aggregator', client.id, 'grouped')
    )
    expect(mocks.syncClient).toHaveBeenCalledWith('internal', client.id, 'grouped')
    await act(async () => finish({ success: true }))
    expect(useMcpGatewayStore.getState().sources.aggregator.busy).toBeNull()
  })

  it('routes restart and port edits only to their selected source', async () => {
    await mount()
    fireEvent.click(section('aggregator').getByRole('button', { name: /Restart/ }))
    await waitFor(() => expect(mocks.restart).toHaveBeenCalledWith('aggregator'))
    const port = section('internal').getByLabelText('Port')
    fireEvent.change(port, { target: { value: '4000' } })
    fireEvent.click(section('internal').getByRole('button', { name: en.gateway.applyPort }))
    await waitFor(() => expect(mocks.setPort).toHaveBeenCalledWith('internal', 4000))
    expect(useMcpGatewayStore.getState().sources.aggregator.view?.port).toBe(3390)
  })

  it('marks legacy client entries as needing an update independently', async () => {
    mocks.clients.mockImplementation(async (source) => ({
      success: true,
      data: [{ ...client, synced: true, upToDate: source === 'aggregator' }]
    }))
    await mount()
    expect(section('internal').getByText(en.gateway.clients.state.stale)).toBeTruthy()
    expect(section('aggregator').getByText(en.gateway.clients.state.connected)).toBeTruthy()
    fireEvent.click(section('internal').getByRole('button', { name: en.gateway.clients.update }))
    await waitFor(() =>
      expect(mocks.syncClient).toHaveBeenCalledWith('internal', client.id, 'grouped')
    )
    expect(mocks.syncClient).toHaveBeenCalledTimes(1)
  })

  it('unsyncs only the selected source key', async () => {
    mocks.clients.mockResolvedValue({
      success: true,
      data: [{ ...client, synced: true, upToDate: true }]
    })
    await mount()
    fireEvent.click(section('aggregator').getByRole('button', { name: en.gateway.clients.remove }))
    await waitFor(() => expect(mocks.unsyncClient).toHaveBeenCalledWith('aggregator', client.id))
    expect(mocks.unsyncClient).toHaveBeenCalledTimes(1)
  })

  it('exports source-specific stdio bridges without a token and withholds unverified HTTP', async () => {
    mocks.connection.mockImplementation(async (source, mode) => ({
      success: true,
      data: { ...connection(source, mode), httpReady: false }
    }))
    await mount()
    for (const source of MCP_GATEWAY_SOURCES) {
      fireEvent.click(section(source).getByText(en.gateway.manual.title))
      expect(section(source).getByText(en.gateway.httpNotReady)).toBeTruthy()
      const snippets = section(source).getAllByRole('button', { name: en.gateway.manual.copy })
      expect(snippets).toHaveLength(2)
      for (const button of snippets) fireEvent.click(button)
    }
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(4))
    const copied = mocks.clipboard.mock.calls.map(([text]) => text)
    expect(copied.join('\n')).not.toContain(SECRET)
    expect(copied[0]).toContain('"se-mcp"')
    expect(copied[2]).toContain('"se-mcp-aggregator"')
    expect(copied[2]).toContain('aggregator')
  })

  it.each<{ name: string; invalidView: McpGatewayView | null }>([
    { name: 'stopped', invalidView: { ...view('internal'), state: 'stopped' } },
    { name: 'failed', invalidView: { ...view('internal'), state: 'error' } },
    { name: 'missing after a failed poll', invalidView: null },
    { name: 'wrong source', invalidView: view('aggregator') },
    {
      name: 'wrong status source',
      invalidView: {
        ...view('internal'),
        status: { ...view('internal').status!, source: 'aggregator' }
      }
    },
    {
      name: 'wrong protocol',
      invalidView: {
        ...view('internal'),
        status: { ...view('internal').status!, protocol: 'other-service' }
      }
    },
    {
      name: 'wrong protocol version',
      invalidView: {
        ...view('internal'),
        status: { ...view('internal').status!, protocolVersion: 2 }
      }
    },
    { name: 'changed port', invalidView: { ...view('internal'), port: 4000 } },
    { name: 'missing port', invalidView: { ...view('internal'), port: null } },
    { name: 'invalid port', invalidView: { ...view('internal'), port: 0 } }
  ])('revokes cached HTTP for a $name view until reverified', async ({ invalidView }) => {
    await mount()
    // A real mode change on the other source must keep working throughout recovery.
    fireEvent.click(section('aggregator').getByRole('button', { name: /Direct/ }))
    await waitFor(() =>
      expect(useMcpGatewayStore.getState().sources.aggregator.connection?.mode).toBe('direct')
    )
    for (const source of MCP_GATEWAY_SOURCES) {
      fireEvent.click(section(source).getByText(en.gateway.manual.title))
    }
    const unaffected = useMcpGatewayStore.getState().sources.aggregator
    const cached = useMcpGatewayStore.getState().sources.internal.connection
    const staleHttpCopy = section('internal').getAllByRole('button', {
      name: en.gateway.manual.copy
    })[2]
    expect(section('internal').getByText(en.gateway.manual.http)).toBeTruthy()

    act(() => {
      // Keep cached httpReady=true to exercise the panel independently of store invalidation.
      patchSource('internal', { view: invalidView })
      // Click before React commits the new view: the clipboard must check live state too.
      fireEvent.click(staleHttpCopy)
    })
    expect(useMcpGatewayStore.getState().sources.internal.connection).toBe(cached)
    expect(mocks.clipboard).not.toHaveBeenCalled()
    expect(section('internal').queryByText(en.gateway.manual.http)).toBeNull()
    expect(section('internal').getByText(en.gateway.httpNotReady)).toBeTruthy()
    expect(screen.getAllByRole('region')[0].textContent).not.toContain(SECRET)
    const offlineCopies = section('internal').getAllByRole('button', {
      name: en.gateway.manual.copy
    })
    expect(offlineCopies).toHaveLength(2)
    for (const button of offlineCopies) fireEvent.click(button)
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(2))
    expect(mocks.clipboard.mock.calls.map(([text]) => text).join('\n')).not.toContain(SECRET)

    act(() =>
      patchSource('internal', {
        view: view('internal'),
        connection: { ...connection('internal'), httpReady: false }
      })
    )
    expect(section('internal').queryByText(en.gateway.manual.http)).toBeNull()
    expect(screen.getAllByRole('region')[0].textContent).not.toContain(SECRET)
    expect(useMcpGatewayStore.getState().sources.aggregator).toBe(unaffected)
    fireEvent.click(
      section('aggregator').getAllByRole('button', { name: en.gateway.manual.copy })[2]
    )
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(3))
    expect(mocks.clipboard.mock.calls[2][0]).toContain('se-mcp-aggregator')
    expect(mocks.clipboard.mock.calls[2][0]).toContain(SECRET)

    act(() => patchSource('internal', { connection: connection('internal') }))
    fireEvent.click(section('internal').getAllByRole('button', { name: en.gateway.manual.copy })[2])
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(4))
    expect(mocks.clipboard.mock.calls[3][0]).toContain('"se-mcp"')
    expect(mocks.clipboard.mock.calls[3][0]).toContain(SECRET)
    expect(useMcpGatewayStore.getState().sources.aggregator).toBe(unaffected)
  })

  it('restores initially unverified HTTP after a verified source refresh', async () => {
    mocks.connection.mockImplementation(async (source, mode) => ({
      success: true,
      data: { ...connection(source, mode), httpReady: source === 'aggregator' }
    }))
    await mount()
    fireEvent.click(section('internal').getByText(en.gateway.manual.title))
    expect(section('internal').queryByText(en.gateway.manual.http)).toBeNull()
    const unaffected = useMcpGatewayStore.getState().sources.aggregator
    mocks.connection.mockImplementation(async (source, mode) => ({
      success: true,
      data: connection(source, mode)
    }))
    await act(async () => useMcpGatewayStore.getState().refreshView('internal'))
    expect(section('internal').getByText(en.gateway.manual.http)).toBeTruthy()
    fireEvent.click(section('internal').getAllByRole('button', { name: en.gateway.manual.copy })[2])
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(1))
    expect(mocks.clipboard.mock.calls[0][0]).toContain(SECRET)
    expect(useMcpGatewayStore.getState().sources.aggregator).toBe(unaffected)
  })

  it('retains current-reference and mode checks while allowing an absent optional status', async () => {
    await mount()
    fireEvent.click(section('internal').getByText(en.gateway.manual.title))
    const staleCopy = section('internal').getAllByRole('button', {
      name: en.gateway.manual.copy
    })[2]
    act(() => {
      patchSource('internal', { connection: connection('internal') })
      fireEvent.click(staleCopy)
    })
    expect(mocks.clipboard).not.toHaveBeenCalled()
    const currentCopy = section('internal').getAllByRole('button', {
      name: en.gateway.manual.copy
    })[2]
    act(() => {
      patchSource('internal', { mode: 'direct' })
      fireEvent.click(currentCopy)
    })
    expect(mocks.clipboard).not.toHaveBeenCalled()
    expect(section('internal').queryByText(en.gateway.manual.http)).toBeNull()

    act(() =>
      patchSource('internal', {
        connection: connection('internal', 'direct'),
        view: { ...view('internal'), status: null }
      })
    )
    fireEvent.click(section('internal').getByText(en.gateway.manual.title))
    expect(section('internal').getByText(en.gateway.manual.http)).toBeTruthy()
    fireEvent.click(section('internal').getAllByRole('button', { name: en.gateway.manual.copy })[2])
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(1))
    expect(mocks.clipboard.mock.calls[0][0]).toContain(SECRET)
  })

  it('does not render or copy a wrong-source connection', async () => {
    mocks.connection.mockImplementation(async (_source, mode) => ({
      success: true,
      data: connection('aggregator', mode)
    }))
    await mount()
    expect(section('internal').queryByText(en.gateway.manual.title)).toBeNull()
    expect(mocks.clipboard).not.toHaveBeenCalled()
  })

  it('allows HTTP only for the verified source and reports safe clipboard failures', async () => {
    await mount()
    fireEvent.click(section('aggregator').getByText(en.gateway.manual.title))
    const buttons = section('aggregator').getAllByRole('button', { name: en.gateway.manual.copy })
    mocks.clipboard.mockRejectedValueOnce(new Error(SECRET))
    fireEvent.click(buttons[0])
    await waitFor(() => expect(mocks.error).toHaveBeenCalledWith(en.gateway.manual.copyFailed))
    fireEvent.click(buttons[2])
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(2))
    expect(mocks.clipboard.mock.calls[1][0]).toContain('se-mcp-aggregator')
    expect(mocks.clipboard.mock.calls[1][0]).toContain(SECRET)
    expect(JSON.stringify(mocks.error.mock.calls)).not.toContain(SECRET)
  })

  it('shows stable action failures rather than raw service errors', async () => {
    mocks.syncClient.mockResolvedValue({ success: false, error: SECRET })
    await mount()
    fireEvent.click(section('internal').getByRole('button', { name: en.gateway.clients.connect }))
    await waitFor(() => expect(section('internal').getByText(en.gateway.error)).toBeTruthy())
    expect(document.body.textContent).not.toContain(SECRET)
  })

  it('does not invoke local APIs or expose copy actions on Web', async () => {
    mocks.desktop = false
    await mount()
    expect(screen.getByText(en.export.desktopOnly)).toBeTruthy()
    for (const fn of [
      mocks.view,
      mocks.connection,
      mocks.clients,
      mocks.read,
      mocks.start,
      mocks.syncClient,
      mocks.clipboard
    ]) {
      expect(fn).not.toHaveBeenCalled()
    }
    expect(screen.queryAllByRole('button')).toHaveLength(0)
  })
})
