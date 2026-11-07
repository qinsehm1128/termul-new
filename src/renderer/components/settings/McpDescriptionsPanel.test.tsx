import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { descriptionLanguage, McpDescriptionsPanel } from './McpDescriptionsPanel'

const api = vi.hoisted(() => ({
  view: vi.fn(),
  servers: vi.fn(),
  descriptions: vi.fn(),
  putDescriptions: vi.fn(),
  describe: vi.fn(),
  describeSettings: vi.fn()
}))
const runtime = vi.hoisted(() => ({ desktop: true }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => runtime.desktop }))
const toast = vi.hoisted(() => ({ error: vi.fn(), success: vi.fn() }))

vi.mock('sonner', () => ({ toast }))
vi.mock('@/lib/mcp-gateway-api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/mcp-gateway-api')>()),
  mcpGatewayApi: api
}))

function row(name: string): HTMLElement {
  const item = screen.getByText(name).closest('li')
  if (!item) throw new Error(`no row for ${name}`)
  return item
}

describe('McpDescriptionsPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    runtime.desktop = true
    api.view.mockResolvedValue({
      success: true,
      data: {
        source: 'aggregator',
        state: 'running',
        port: 3290,
        settingsPath: '/x',
        status: {
          source: 'aggregator',
          protocol: 'se-mcp-gateway',
          protocolVersion: 1,
          version: '1',
          pid: 1,
          port: 3290,
          startedAt: 1,
          configRevision: 1,
          configError: null,
          upstreams: [{ id: 'id-c7', name: 'context7', state: 'connected' }],
          builtIns: []
        }
      }
    })
    api.servers.mockResolvedValue({
      success: true,
      data: {
        servers: [
          { name: 'session-memory', description: 'memory', builtIn: true },
          { name: 'context7', description: 'Library docs', builtIn: false }
        ]
      }
    })
    api.descriptions.mockResolvedValue({ success: true, data: {} })
    api.putDescriptions.mockResolvedValue({ success: true, data: undefined })
    api.describeSettings.mockResolvedValue({ success: false, code: 'INVOKE_ERROR' })
    api.describe.mockResolvedValue({
      success: true,
      data: [{ name: 'context7', id: 'id-c7', description: '查询库文档' }]
    })
  })

  it('lists upstream servers only, with what agents read', async () => {
    render(<McpDescriptionsPanel />)
    expect(await screen.findByText('Library docs')).toBeTruthy()
    expect(screen.queryByText('session-memory')).toBeNull()
    expect(api.view).toHaveBeenCalledWith('aggregator')
    expect(api.servers).toHaveBeenCalledWith('aggregator')
  })

  it('stores an edited line under the server config id', async () => {
    render(<McpDescriptionsPanel />)
    await screen.findByText('context7')
    fireEvent.click(within(row('context7')).getByRole('button', { name: /Edit the description/ }))
    fireEvent.change(screen.getByLabelText('Description of context7'), {
      target: { value: 'Look up current library docs' }
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() =>
      expect(api.putDescriptions).toHaveBeenCalledWith({
        'id-c7': 'Look up current library docs'
      })
    )
  })

  it('asks the model in the UI language and explains a missing model', async () => {
    render(<McpDescriptionsPanel />)
    await screen.findByText('context7')
    fireEvent.click(within(row('context7')).getByRole('button', { name: /Describe context7/ }))
    await waitFor(() => expect(api.describe).toHaveBeenCalledWith('English', ['context7']))

    api.describe.mockResolvedValueOnce({ success: false, code: 'AI_NOT_CONFIGURED' })
    fireEvent.click(screen.getByRole('button', { name: 'Describe all with AI' }))
    await waitFor(() => expect(api.describe).toHaveBeenLastCalledWith('English', undefined))
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(expect.stringContaining('AI channels page'))
    )
  })

  it('retains the external directory when the aggregator is stopped', async () => {
    api.view.mockResolvedValueOnce({
      success: true,
      data: { source: 'aggregator', state: 'stopped', port: 3390, settingsPath: '/x' }
    })
    render(<McpDescriptionsPanel />)
    expect(await screen.findByText('Library docs')).toBeTruthy()
    expect(screen.getByText('context7')).toBeTruthy()
    expect(api.servers).toHaveBeenCalledWith('aggregator')
  })

  it('does not surface raw generation errors or leak them to logs', async () => {
    api.describe.mockResolvedValueOnce({
      success: false,
      code: 'INVOKE_ERROR',
      error: 'SECRET_TOKEN'
    })
    render(<McpDescriptionsPanel />)
    await screen.findByText('context7')
    fireEvent.click(screen.getByRole('button', { name: 'Describe all with AI' }))
    await waitFor(() => expect(toast.error).toHaveBeenCalled())
    expect(JSON.stringify(toast.error.mock.calls)).not.toContain('SECRET_TOKEN')
  })

  it('does not read local gateway or description APIs on Web', () => {
    runtime.desktop = false
    render(<McpDescriptionsPanel />)
    expect(api.view).not.toHaveBeenCalled()
    expect(api.servers).not.toHaveBeenCalled()
    expect(api.descriptions).not.toHaveBeenCalled()
    expect(api.describeSettings).not.toHaveBeenCalled()
  })

  it('maps Chinese UI languages to Chinese descriptions', () => {
    expect(descriptionLanguage('zh-CN')).toBe('Chinese')
    expect(descriptionLanguage('en')).toBe('English')
  })
})
