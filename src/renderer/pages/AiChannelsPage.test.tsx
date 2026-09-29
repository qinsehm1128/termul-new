import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AiChannelsDocument } from '@/lib/ai-channels-api'

const api = vi.hoisted(() => ({
  load: vi.fn(),
  save: vi.fn(),
  setKey: vi.fn(),
  models: vi.fn(),
  test: vi.fn()
}))

vi.mock('sonner', () => ({ toast: { error: vi.fn(), success: vi.fn() } }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))
vi.mock('@/lib/ai-channels-api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/ai-channels-api')>()),
  aiChannelsApi: api
}))

import AiChannelsPage from './AiChannelsPage'

const relay: AiChannelsDocument = {
  schemaVersion: 2,
  channels: [
    {
      id: 'relay',
      name: 'Relay',
      api: 'anthropic-messages',
      baseUrl: 'https://relay.test',
      enabled: true,
      models: ['claude-sonnet-5']
    }
  ],
  purposes: {}
}

function card(name: string): HTMLElement {
  return screen.getByRole('region', { name })
}

describe('AI channels page', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.load.mockResolvedValue({ success: true, data: { document: relay, keys: { relay: true } } })
    api.save.mockResolvedValue({ success: true, data: undefined })
    api.setKey.mockResolvedValue({ success: true, data: undefined })
    api.models.mockResolvedValue({ success: true, data: ['claude-opus-5', 'claude-sonnet-5'] })
    api.test.mockResolvedValue({ success: true, data: { reply: 'OK', millis: 120 } })
  })

  it('adds a preset channel and saves the document with its protocol and URL', async () => {
    render(<AiChannelsPage />)
    await screen.findByDisplayValue('Relay')
    fireEvent.click(screen.getByRole('button', { name: 'OpenAI' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(api.save).toHaveBeenCalledTimes(1))
    const saved = api.save.mock.calls[0][0] as AiChannelsDocument
    expect(saved.channels[1]).toMatchObject({
      name: 'OpenAI',
      api: 'openai-responses',
      baseUrl: 'https://api.openai.com/v1'
    })
    expect(saved.channels[1].id).toMatch(/^[A-Za-z][A-Za-z0-9._-]{0,63}$/)
  })

  it('stores and removes a key without putting it in the document', async () => {
    render(<AiChannelsPage />)
    await screen.findByDisplayValue('Relay')
    const relayCard = card('Relay')
    fireEvent.change(within(relayCard).getByLabelText('API key'), {
      target: { value: 'sk-secret' }
    })
    fireEvent.click(within(relayCard).getByRole('button', { name: 'Save key' }))
    await waitFor(() => expect(api.setKey).toHaveBeenCalledWith('relay', 'sk-secret'))
    fireEvent.click(within(relayCard).getByRole('button', { name: 'Remove key' }))
    await waitFor(() => expect(api.setKey).toHaveBeenCalledWith('relay', null))
    await waitFor(() => expect(within(relayCard).getByText('not set')).toBeTruthy())
    expect(api.save).not.toHaveBeenCalled()
  })

  it('adds a discovered model and tests the channel with the chosen model', async () => {
    render(<AiChannelsPage />)
    await screen.findByDisplayValue('Relay')
    const relayCard = card('Relay')
    fireEvent.click(within(relayCard).getByRole('button', { name: 'Fetch from server' }))
    fireEvent.click(await within(relayCard).findByRole('button', { name: '+ claude-opus-5' }))
    expect(
      within(relayCard).getByRole('button', { name: 'Remove model claude-opus-5' })
    ).toBeTruthy()

    fireEvent.click(within(relayCard).getByRole('button', { name: 'Test' }))
    await waitFor(() =>
      expect(api.test).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'relay', models: ['claude-sonnet-5', 'claude-opus-5'] }),
        'claude-sonnet-5'
      )
    )
    expect(await within(relayCard).findByText('Works (120 ms): OK')).toBeTruthy()
  })

  it('picks the channel and model that write MCP server descriptions', async () => {
    render(<AiChannelsPage />)
    await screen.findByDisplayValue('Relay')
    fireEvent.change(screen.getByLabelText('Channel'), { target: { value: 'relay' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(api.save).toHaveBeenCalledTimes(1))
    expect((api.save.mock.calls[0][0] as AiChannelsDocument).purposes.mcpSummary).toEqual({
      channelId: 'relay',
      model: 'claude-sonnet-5'
    })
  })
})
