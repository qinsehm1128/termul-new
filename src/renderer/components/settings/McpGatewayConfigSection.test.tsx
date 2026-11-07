import type { McpGatewaySource } from '@shared/types/mcp-gateway.types'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mcpGatewayApi } from '@/lib/mcp-gateway-api'
import { McpGatewayConfigSection, McpGatewaySourceConfigCard } from './McpGatewayConfigSection'

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  desktop: vi.fn(),
  clipboard: vi.fn(),
  toast: vi.fn(),
  log: vi.fn()
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: mocks.desktop }))
vi.mock('@/lib/api', () => ({ clipboardApi: { writeText: mocks.clipboard } }))
vi.mock('@/hooks/use-toast', () => ({ toast: mocks.toast }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: mocks.log }))

const token = 'TOKEN_SENTINEL_NEVER_EXPORT'
const bridgeFor = (source: McpGatewaySource) => ({
  command: 'C:\\Apps With Spaces\\Se\\se-mcp.exe',
  args: [
    '--source',
    source,
    '--state-root',
    'C:\\State With Spaces\\Se',
    '--mode',
    'direct',
    'quote"and\\slash'
  ]
})
const bridge = bridgeFor('internal')
function connection(source: McpGatewaySource = 'internal') {
  return {
    success: true as const,
    data: {
      source,
      mode: 'direct' as const,
      bridge: {
        ...bridgeFor(source),
        env: { AUTHORIZATION: token },
        settings: 'PRIVATE_SETTINGS_SENTINEL'
      },
      httpReady: false,
      bridgeAvailable: true,
      token,
      url: 'http://localhost.invalid/mcp',
      port: 1234,
      settingsPath: 'PRIVATE_SETTINGS_SENTINEL'
    }
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
const preview = () => screen.getByRole('button', { name: 'Preview' })
const copy = () => screen.getByRole('button', { name: 'Copy configuration' })
const toml = () => screen.getByRole('button', { name: 'Codex TOML' })

beforeEach(() => {
  vi.restoreAllMocks()
  vi.clearAllMocks()
  mocks.desktop.mockReturnValue(true)
  mocks.invoke.mockImplementation((_command, args) => Promise.resolve(connection(args.source)))
  mocks.clipboard.mockResolvedValue({ success: true })
  mocks.log.mockResolvedValue(undefined)
})

describe('McpGatewayConfigSection', () => {
  it('is explicit about source-isolated direct mode, permissions, and upstream loop safety; loads only on demand', () => {
    render(<McpGatewaySourceConfigCard source="internal" />)
    expect(screen.getByRole('heading', { name: 'Se built-in MCP' })).toBeInTheDocument()
    expect(screen.getByText('se-mcp · Direct mode · local stdio export')).toBeInTheDocument()
    expect(screen.getByText(/Tool visibility is not authorization/)).toBeInTheDocument()
    expect(screen.getByText(/Do not import this entry into Se's upstream/)).toBeInTheDocument()
    expect(mocks.invoke).not.toHaveBeenCalled()
  })

  it('previews and copies exact JSON bridge command/args without token or lifecycle mutations', async () => {
    const { container } = render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(preview())
    await waitFor(() => expect(container.querySelector('pre')).not.toBeNull())
    const expected = JSON.stringify({ mcpServers: { 'se-mcp': bridge } }, null, 2)
    expect(container.querySelector('pre')?.textContent).toBe(expected)
    expect(mocks.clipboard).not.toHaveBeenCalled()
    fireEvent.click(copy())
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledWith(expected))
    await waitFor(() =>
      expect(mocks.toast).toHaveBeenCalledWith({ title: 'MCP connection configuration copied' })
    )
    expect(mocks.invoke.mock.calls).toEqual([
      ['mcp_connection_info', { source: 'internal', mode: 'direct' }],
      ['mcp_connection_info', { source: 'internal', mode: 'direct' }]
    ])
    expect(container.textContent).not.toContain(token)
    expect(JSON.stringify(mocks.toast.mock.calls)).not.toContain(token)
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain(token)
    expect(JSON.stringify(mocks.clipboard.mock.calls)).not.toContain(token)
  })

  it('clears preview on format switch and preserves exact TOML quoting, spaces, and backslashes', async () => {
    const { container } = render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(preview())
    await waitFor(() => expect(container.querySelector('pre')).not.toBeNull())
    fireEvent.click(toml())
    expect(container.querySelector('pre')).toBeNull()
    fireEvent.click(copy())
    const expected = [
      '[mcp_servers.se-mcp]',
      `command = ${JSON.stringify(bridge.command)}`,
      `args = [${bridge.args.map((arg) => JSON.stringify(arg)).join(', ')}]`,
      'tool_timeout_sec = 300'
    ].join('\n')
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledWith(expected))
    expect(container.querySelector('pre')?.textContent).toBe(expected)
    expect(expected).not.toContain(token)
  })

  it.each([
    [
      'unavailable bridge',
      { success: true, data: { ...connection().data, bridgeAvailable: false } }
    ],
    ['missing bridge', { success: true, data: { ...connection().data, bridge: undefined } }],
    [
      'empty command',
      { success: true, data: { ...connection().data, bridge: { command: '', args: [] } } }
    ]
  ])('does not export a %s', async (_label, result) => {
    mocks.invoke.mockResolvedValue(result)
    const { container } = render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(copy())
    await screen.findByRole('alert')
    expect(container.querySelector('pre')).toBeNull()
    expect(mocks.clipboard).not.toHaveBeenCalled()
    expect(mocks.toast).toHaveBeenCalledWith(expect.objectContaining({ variant: 'destructive' }))
    expect(mocks.log).toHaveBeenCalledWith({
      source: 'McpGatewayConfigSection',
      message: 'MCP stdio export failed: internal direct bridgeMissing'
    })
  })

  it.each([
    'failure',
    'invoke throw',
    'API throw'
  ])('never falls back to stale preview after %s', async (failure) => {
    const { container } = render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(preview())
    await waitFor(() => expect(container.querySelector('pre')).not.toBeNull())
    const sensitiveError = new Error(`${token} ${bridge.command} ${JSON.stringify(bridge.args)}`)
    if (failure === 'failure')
      mocks.invoke.mockResolvedValue({ success: false, error: sensitiveError.message })
    else if (failure === 'invoke throw') mocks.invoke.mockRejectedValue(sensitiveError)
    else vi.spyOn(mcpGatewayApi, 'connection').mockRejectedValue(sensitiveError)
    fireEvent.click(copy())
    await screen.findByRole('alert')
    expect(container.querySelector('pre')).toBeNull()
    expect(mocks.clipboard).not.toHaveBeenCalled()
    const output = JSON.stringify([
      container.textContent,
      mocks.toast.mock.calls,
      mocks.log.mock.calls
    ])
    expect(output).not.toContain(token)
    expect(output).not.toContain(bridge.command)
    expect(mocks.log).toHaveBeenCalledWith({
      source: 'McpGatewayConfigSection',
      message: 'MCP stdio export failed: internal direct loadFailed'
    })
  })

  it.each([
    'failure',
    'throw'
  ])('does not report successful copy on clipboard %s', async (failure) => {
    if (failure === 'failure') mocks.clipboard.mockResolvedValue({ success: false, error: token })
    else mocks.clipboard.mockRejectedValue(new Error(token))
    render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(copy())
    await screen.findByRole('alert')
    expect(mocks.toast).toHaveBeenCalledTimes(1)
    expect(mocks.toast).toHaveBeenCalledWith({
      title: 'Unable to copy the configuration. Please try again.',
      variant: 'destructive'
    })
    expect(JSON.stringify([mocks.toast.mock.calls, mocks.log.mock.calls])).not.toContain(token)
  })

  it('ignores an obsolete copy result after format changes while a new preview is loading', async () => {
    const old = deferred<ReturnType<typeof connection>>()
    mocks.invoke.mockReturnValueOnce(old.promise)
    const { container } = render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(copy())
    fireEvent.click(toml())
    expect(container.querySelector('pre')).toBeNull()
    fireEvent.click(preview())
    await waitFor(() =>
      expect(container.querySelector('pre')?.textContent).toContain('[mcp_servers.se-mcp]')
    )
    await act(async () => old.resolve(connection()))
    expect(container.querySelector('pre')?.textContent).toContain('[mcp_servers.se-mcp]')
    expect(mocks.clipboard).not.toHaveBeenCalled()
    expect(mocks.toast).not.toHaveBeenCalled()
  })

  it('invalidates pending clipboard completion on format changes', async () => {
    const pending = deferred<{ success: boolean }>()
    mocks.clipboard.mockReturnValue(pending.promise)
    const { container } = render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(copy())
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(1))
    expect(copy()).toBeDisabled()
    fireEvent.click(toml())
    expect(container.querySelector('pre')).toBeNull()
    await act(async () => pending.resolve({ success: true }))
    expect(mocks.toast).not.toHaveBeenCalled()
  })

  it.each(['connection', 'clipboard'])('ignores %s resolution after unmount', async (stage) => {
    const pendingConnection = deferred<ReturnType<typeof connection>>()
    const pendingClipboard = deferred<{ success: boolean }>()
    if (stage === 'connection') mocks.invoke.mockReturnValue(pendingConnection.promise)
    else mocks.clipboard.mockReturnValue(pendingClipboard.promise)
    const view = render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(copy())
    if (stage === 'clipboard') await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(1))
    view.unmount()
    await act(async () => {
      pendingConnection.resolve(connection())
      pendingClipboard.resolve({ success: true })
    })
    if (stage === 'connection') expect(mocks.clipboard).not.toHaveBeenCalled()
    expect(mocks.toast).not.toHaveBeenCalled()
    expect(mocks.log).toHaveBeenCalledTimes(1)
    expect(mocks.log).toHaveBeenCalledWith({
      level: 'warn',
      source: 'McpGatewayConfigSection',
      message: 'MCP stdio export requested: internal direct'
    })
  })

  it.each([
    'internal',
    'aggregator'
  ] as const)('exports exact JSON and TOML keys and source arguments for %s', async (source) => {
    const key = source === 'internal' ? 'se-mcp' : 'se-mcp-aggregator'
    const sourceBridge = bridgeFor(source)
    const { container } = render(<McpGatewaySourceConfigCard source={source} />)
    fireEvent.click(copy())
    const json = JSON.stringify({ mcpServers: { [key]: sourceBridge } }, null, 2)
    await waitFor(() => expect(mocks.clipboard).toHaveBeenLastCalledWith(json))
    fireEvent.click(toml())
    expect(container.querySelector('pre')).toBeNull()
    fireEvent.click(copy())
    const expectedToml = [
      `[mcp_servers.${key}]`,
      `command = ${JSON.stringify(sourceBridge.command)}`,
      `args = [${sourceBridge.args.map((arg) => JSON.stringify(arg)).join(', ')}]`,
      'tool_timeout_sec = 300'
    ].join('\n')
    await waitFor(() => expect(mocks.clipboard).toHaveBeenLastCalledWith(expectedToml))
    expect(mocks.invoke.mock.calls).toEqual([
      ['mcp_connection_info', { source, mode: 'direct' }],
      ['mcp_connection_info', { source, mode: 'direct' }]
    ])
    const output = JSON.stringify([
      container.textContent,
      mocks.clipboard.mock.calls,
      mocks.log.mock.calls,
      mocks.toast.mock.calls
    ])
    expect(output).not.toContain(token)
    expect(output).not.toContain('PRIVATE_SETTINGS_SENTINEL')
    expect(output).not.toContain('localhost.invalid')
    expect(output).not.toContain('se-memory')
  })

  it('shows two bounded sources, with independent loading and formats and no auto lifecycle calls', async () => {
    const old = deferred<ReturnType<typeof connection>>()
    mocks.invoke.mockImplementation((_command, args) =>
      args.source === 'internal' ? old.promise : Promise.resolve(connection('aggregator'))
    )
    const { container } = render(<McpGatewayConfigSection />)
    const internal = within(screen.getByRole('region', { name: 'Se built-in MCP' }))
    const aggregator = within(screen.getByRole('region', { name: 'External MCP aggregator' }))
    expect(container.textContent).toContain('No external upstream tools')
    expect(container.textContent).toContain('No Se built-in memory')
    expect(container.textContent).not.toMatch(/Full Se MCP|full.entry|se-memory/i)
    expect(mocks.invoke).not.toHaveBeenCalled()
    fireEvent.click(internal.getByRole('button', { name: 'Preview' }))
    expect(internal.getByRole('button', { name: 'Copy configuration' })).toBeDisabled()
    expect(aggregator.getByRole('button', { name: 'Copy configuration' })).toBeEnabled()
    fireEvent.click(aggregator.getByRole('button', { name: 'Codex TOML' }))
    fireEvent.click(aggregator.getByRole('button', { name: 'Copy configuration' }))
    await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(1))
    expect(mocks.clipboard.mock.calls[0][0]).toContain('[mcp_servers.se-mcp-aggregator]')
    expect(internal.getByRole('button', { name: 'MCP JSON' })).toHaveAttribute(
      'aria-pressed',
      'true'
    )
    await act(async () => old.resolve(connection()))
    expect(internal.getByText(/"mcpServers"/)).toBeInTheDocument()
    expect(mocks.invoke.mock.calls.map(([command]) => command)).toEqual([
      'mcp_connection_info',
      'mcp_connection_info'
    ])
  })

  it.each([
    'preview',
    'connection',
    'clipboard'
  ])('clears old source state and ignores stale %s completion after switching source', async (stage) => {
    const pendingConnection = deferred<ReturnType<typeof connection>>()
    const pendingClipboard = deferred<{ success: boolean }>()
    if (stage === 'connection') mocks.invoke.mockReturnValueOnce(pendingConnection.promise)
    if (stage === 'clipboard') mocks.clipboard.mockReturnValueOnce(pendingClipboard.promise)
    const view = render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(stage === 'preview' ? preview() : copy())
    if (stage === 'preview')
      await waitFor(() => expect(view.container.querySelector('pre')).not.toBeNull())
    if (stage === 'clipboard') await waitFor(() => expect(mocks.clipboard).toHaveBeenCalledTimes(1))
    view.rerender(<McpGatewaySourceConfigCard source="aggregator" />)
    expect(view.container.querySelector('pre')).toBeNull()
    expect(copy()).toBeEnabled()
    fireEvent.click(toml())
    fireEvent.click(preview())
    await waitFor(() =>
      expect(view.container.querySelector('pre')?.textContent).toContain(
        '[mcp_servers.se-mcp-aggregator]'
      )
    )
    await act(async () => {
      pendingConnection.resolve(connection())
      pendingClipboard.resolve({ success: true })
    })
    expect(view.container.querySelector('pre')?.textContent).toContain(
      '[mcp_servers.se-mcp-aggregator]'
    )
    expect(mocks.clipboard).toHaveBeenCalledTimes(stage === 'clipboard' ? 1 : 0)
    expect(mocks.toast).not.toHaveBeenCalled()
  })

  it.each([
    ['wrong source', { ...connection().data, source: 'aggregator' }],
    ['missing source', { ...connection().data, source: undefined }],
    ['wrong mode', { ...connection().data, mode: 'entry' }]
  ])('rejects a %s response without copying', async (_label, data) => {
    mocks.invoke.mockResolvedValue({ success: true, data })
    const { container } = render(<McpGatewaySourceConfigCard source="internal" />)
    fireEvent.click(copy())
    await screen.findByRole('alert')
    expect(container.querySelector('pre')).toBeNull()
    expect(mocks.clipboard).not.toHaveBeenCalled()
  })

  it.each([
    'browser',
    'shared-live',
    'standalone'
  ])('disables local export in %s without invoking a desktop stub', (mode) => {
    mocks.desktop.mockReturnValue(false)
    mocks.invoke.mockImplementation(() => {
      throw new Error(`desktop stub: ${mode}`)
    })
    const connectionSpy = vi.spyOn(mcpGatewayApi, 'connection')
    render(<McpGatewayConfigSection />)
    expect(screen.getAllByRole('status')).toHaveLength(2)
    for (const status of screen.getAllByRole('status'))
      expect(status).toHaveTextContent('Desktop only')
    for (const button of screen.getAllByRole('button')) {
      expect(button).toBeDisabled()
      fireEvent.click(button)
    }
    expect(connectionSpy).not.toHaveBeenCalled()
    expect(mocks.invoke).not.toHaveBeenCalled()
    expect(mocks.clipboard).not.toHaveBeenCalled()
  })
})
