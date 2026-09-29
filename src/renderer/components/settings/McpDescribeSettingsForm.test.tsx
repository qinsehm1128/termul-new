import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { McpDescribeSettings } from '@/lib/mcp-gateway-api'
import { McpDescribeSettingsForm } from './McpDescribeSettingsForm'

const api = vi.hoisted(() => ({ describeSettings: vi.fn(), putDescribeSettings: vi.fn() }))
const toast = vi.hoisted(() => ({ error: vi.fn(), success: vi.fn() }))

vi.mock('sonner', () => ({ toast }))
vi.mock('@/lib/mcp-gateway-api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/mcp-gateway-api')>()),
  mcpGatewayApi: api
}))

const defaults: McpDescribeSettings = {
  prompt: 'Default prompt, at most {maxChars} characters.',
  maxChars: 300,
  toolDescriptionChars: 0,
  maxAnswerTokens: 1024
}

async function open(): Promise<void> {
  render(<McpDescribeSettingsForm />)
  fireEvent.click(screen.getByRole('button', { name: 'Generation settings' }))
  await screen.findByLabelText('Prompt')
}

describe('McpDescribeSettingsForm', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.describeSettings.mockResolvedValue({
      success: true,
      data: { settings: { ...defaults, prompt: 'Mine', maxChars: 500 }, defaults }
    })
    api.putDescribeSettings.mockResolvedValue({ success: true, data: undefined })
  })

  it('saves the edited prompt and limits as numbers', async () => {
    await open()
    expect((screen.getByLabelText('Prompt') as HTMLTextAreaElement).value).toBe('Mine')
    fireEvent.change(screen.getByLabelText('Prompt'), { target: { value: 'Say it in {maxChars}' } })
    fireEvent.change(screen.getByLabelText('Tool description length'), {
      target: { value: '1a50' }
    })
    fireEvent.change(screen.getByLabelText('Answer budget'), { target: { value: '4096' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save settings' }))
    await waitFor(() =>
      expect(api.putDescribeSettings).toHaveBeenCalledWith({
        prompt: 'Say it in {maxChars}',
        maxChars: 500,
        toolDescriptionChars: 150,
        maxAnswerTokens: 4096
      })
    )
    expect(toast.success).toHaveBeenCalled()
  })

  it('restores the built-in defaults', async () => {
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Restore defaults' }))
    await waitFor(() => expect(api.putDescribeSettings).toHaveBeenCalledWith(defaults))
    await waitFor(() =>
      expect((screen.getByLabelText('Description length') as HTMLInputElement).value).toBe('300')
    )
  })

  it('shows why the host refused the settings', async () => {
    api.putDescribeSettings.mockResolvedValue({
      success: false,
      error: 'the description length must be 20–2000 characters',
      code: 'MCP_DESCRIBE_SETTINGS_INVALID'
    })
    await open()
    fireEvent.click(screen.getByRole('button', { name: 'Save settings' }))
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith('the description length must be 20–2000 characters')
    )
    expect(toast.success).not.toHaveBeenCalled()
  })
})
