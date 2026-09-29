import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useMcpGatewayStore } from '@/stores/mcp-gateway-store'
import { descriptionLanguage, McpDescriptionsPanel } from './McpDescriptionsPanel'

const api = vi.hoisted(() => ({
  servers: vi.fn(),
  descriptions: vi.fn(),
  putDescriptions: vi.fn(),
  describe: vi.fn()
}))
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
    useMcpGatewayStore.setState({
      view: {
        state: 'running',
        port: 3290,
        settingsPath: '/x',
        status: {
          version: '1',
          pid: 1,
          port: 3290,
          startedAt: 1,
          configRevision: 1,
          configError: null,
          upstreams: [{ id: 'id-c7', name: 'context7', state: 'connected' }],
          builtIns: ['session-memory']
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
    api.describe.mockResolvedValue({
      success: true,
      data: [{ name: 'context7', id: 'id-c7', description: '查询库文档' }]
    })
  })

  it('lists upstream servers only, with what agents read', async () => {
    render(<McpDescriptionsPanel />)
    expect(await screen.findByText('Library docs')).toBeTruthy()
    expect(screen.queryByText('session-memory')).toBeNull()
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

  it('maps Chinese UI languages to Chinese descriptions', () => {
    expect(descriptionLanguage('zh-CN')).toBe('Chinese')
    expect(descriptionLanguage('en')).toBe('English')
  })
})
