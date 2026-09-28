import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { emptyMcpConfig } from '@/lib/mcp-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { useMcpStore } from '@/stores/mcp-store'
import { McpServersSettings } from './McpServersSettings'

const { toastError, toastSuccess, beginMcpOAuth, cancelMcpOAuth, listen } = vi.hoisted(() => ({
  toastError: vi.fn(),
  toastSuccess: vi.fn(),
  beginMcpOAuth: vi.fn(),
  cancelMcpOAuth: vi.fn(),
  listen: vi.fn()
}))

vi.mock('sonner', () => ({ toast: { error: toastError, success: toastSuccess } }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: vi.fn(() => true) }))
vi.mock('@/lib/tauri-event', () => ({ listen }))
vi.mock('@/lib/mcp-api', async () => {
  const actual = await vi.importActual<typeof import('@/lib/mcp-api')>('@/lib/mcp-api')
  return { ...actual, beginMcpOAuth, cancelMcpOAuth }
})

const probeMcpServer = vi.fn(async () => {})
const reloadMcpConfig = vi.fn(async () => {})
const reloadMcpStatus = vi.fn(async () => {})
const handlers = new Map<
  string,
  (event: { payload: { serverId?: string; success?: boolean } }) => void
>()

function seedStore(server: Record<string, unknown>): void {
  useMcpStore.setState({
    config: { ...emptyMcpConfig(), upstreams: [server] as never },
    probeStatus: {},
    probeError: {},
    tools: {},
    toolsLoaded: {},
    probing: {},
    load: reloadMcpConfig,
    loadStatus: reloadMcpStatus,
    probe: probeMcpServer,
    saveUpstream: vi.fn(async () => {}),
    importUpstreams: vi.fn(async () => {}),
    setUpstreamEnabled: vi.fn(async () => {}),
    deleteUpstream: vi.fn(async () => {}),
    loadTools: vi.fn(async () => {})
  })
}

describe('McpServersSettings OAuth runtime loop', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    handlers.clear()
    vi.mocked(isTauriContext).mockReturnValue(true)
    listen.mockImplementation(
      async (event: string, handler: (event: { payload: unknown }) => void) => {
        handlers.set(
          event,
          handler as (event: { payload: { serverId?: string; success?: boolean } }) => void
        )
        return () => {
          handlers.delete(event)
        }
      }
    )
    beginMcpOAuth.mockResolvedValue({
      success: true,
      data: {
        serverId: 'remote',
        authorizationUrl: 'http://127.0.0.1:9/authorize',
        expiresAt: 10,
        state: 'pending'
      }
    })
    cancelMcpOAuth.mockResolvedValue({
      success: true,
      data: {
        serverId: 'remote',
        state: 'idle',
        hasCredentials: true,
        canRefresh: true,
        authorizationRequired: false
      }
    })
    seedStore({
      id: 'remote',
      type: 'http',
      name: 'Remote',
      url: 'https://mcp.example.test/mcp',
      enabled: true,
      oauth: {
        authMode: 'oauth',
        registrationMode: 'dynamic',
        endpoints: { resource: 'https://mcp.example.test/mcp' },
        redirectUri: 'http://127.0.0.1:9/oauth/callback'
      }
    })
  })

  it('reloads config, status, and probe after the desktop completion event', async () => {
    render(<McpServersSettings />)
    fireEvent.click(screen.getByRole('button', { name: 'Authorize Remote with OAuth' }))
    await waitFor(() => expect(beginMcpOAuth).toHaveBeenCalledWith('remote'))
    expect(
      screen.getByRole('button', { name: 'Cancel OAuth authorization for Remote' })
    ).toBeInTheDocument()

    handlers.get('mcp-oauth-completed')?.({ payload: { serverId: 'remote', success: true } })

    await waitFor(() => expect(reloadMcpConfig).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(reloadMcpStatus).toHaveBeenCalledTimes(1))
    expect(probeMcpServer).toHaveBeenCalledWith('remote')
    expect(toastSuccess).toHaveBeenCalledWith('OAuth authorization completed.')
    expect(JSON.stringify(toastSuccess.mock.calls)).not.toContain('access_token')
  })

  it('keeps a retry affordance when authorization fails and cancels only the pending flow', async () => {
    render(<McpServersSettings />)
    handlers.get('mcp-oauth-completed')?.({ payload: { serverId: 'remote', success: false } })
    expect(await screen.findByRole('status')).toHaveTextContent(
      'OAuth authorization failed. Retry to start again.'
    )
    expect(
      screen.getByRole('button', { name: 'Retry OAuth authorization for Remote' })
    ).toBeEnabled()
    expect(reloadMcpConfig).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Retry OAuth authorization for Remote' }))
    await waitFor(() => expect(beginMcpOAuth).toHaveBeenCalledTimes(1))
    fireEvent.click(screen.getByRole('button', { name: 'Cancel OAuth authorization for Remote' }))
    await waitFor(() => expect(cancelMcpOAuth).toHaveBeenCalledWith('remote'))
    await waitFor(() =>
      expect(toastSuccess).toHaveBeenCalledWith(
        'OAuth authorization cancelled. Existing credentials were kept.'
      )
    )
    expect(
      screen.queryByRole('button', { name: 'Cancel OAuth authorization for Remote' })
    ).toBeNull()
  })

  it('does not start OAuth for legacy SSE or the web runtime', () => {
    seedStore({
      id: 'legacy',
      type: 'sse',
      name: 'Legacy',
      url: 'https://mcp.example.test/sse',
      enabled: true
    })
    const { unmount } = render(<McpServersSettings />)
    const sse = screen.getByRole('button', {
      name: 'Legacy SSE cannot use OAuth for Legacy. Switch the transport to HTTP.'
    })
    expect(sse).toBeDisabled()
    fireEvent.click(sse)
    expect(beginMcpOAuth).not.toHaveBeenCalled()
    unmount()

    listen.mockClear()
    vi.mocked(isTauriContext).mockReturnValue(false)
    seedStore({
      id: 'remote',
      type: 'http',
      name: 'Remote',
      url: 'https://mcp.example.test/mcp',
      enabled: true
    })
    render(<McpServersSettings />)
    const desktopOnly = screen.getByRole('button', {
      name: 'OAuth authorization for Remote is available in the desktop app.'
    })
    expect(desktopOnly).toBeDisabled()
    fireEvent.click(desktopOnly)
    expect(beginMcpOAuth).not.toHaveBeenCalled()
    expect(listen).not.toHaveBeenCalled()
  })

  it('preserves resource and redirect metadata in the edit JSON', () => {
    render(<McpServersSettings />)
    fireEvent.click(screen.getByRole('button', { name: /edit remote/i }))
    const edited = JSON.parse((screen.getByLabelText('MCP JSON') as HTMLTextAreaElement).value)
    expect(edited.oauth).toEqual({
      authMode: 'oauth',
      registrationMode: 'dynamic',
      endpoints: { resource: 'https://mcp.example.test/mcp' },
      redirectUri: 'http://127.0.0.1:9/oauth/callback'
    })
    expect(JSON.stringify(edited)).not.toContain('accessToken')
  })
})
