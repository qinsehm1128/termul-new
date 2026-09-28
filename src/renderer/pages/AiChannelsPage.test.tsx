import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const emptyDocument = () => ({
  schemaVersion: 1 as const,
  revision: 1,
  channels: [],
  profiles: [],
  routes: []
})

const loadAiChannels = vi.fn()
const saveAiChannels = vi.fn()
const getAiChannelCredentialStatus = vi.fn()
const setAiChannelCredential = vi.fn()
const deleteAiChannelCredential = vi.fn()

vi.mock('@/lib/ai-channels-persistence', () => ({
  emptyAiChannelsDocument: () => emptyDocument(),
  loadAiChannels: () => loadAiChannels(),
  saveAiChannels: (document: unknown) => saveAiChannels(document),
  getAiChannelCredentialStatus: (id: string) => getAiChannelCredentialStatus(id),
  setAiChannelCredential: (channel: unknown, secret: string) =>
    setAiChannelCredential(channel, secret),
  deleteAiChannelCredential: (id: string) => deleteAiChannelCredential(id)
}))

vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))

import AiChannelsPage from './AiChannelsPage'

describe('AI Channels page', () => {
  beforeEach(() => {
    loadAiChannels.mockResolvedValue(emptyDocument())
    saveAiChannels.mockResolvedValue(undefined)
    getAiChannelCredentialStatus.mockResolvedValue({
      success: true,
      data: { hasCredential: false }
    })
    setAiChannelCredential.mockResolvedValue(undefined)
    deleteAiChannelCredential.mockResolvedValue(undefined)
  })

  it('keeps AI channels independent from app preferences and persists nonsecret configuration', async () => {
    render(<AiChannelsPage />)
    expect(
      await screen.findByRole('heading', { name: 'AI Channels', level: 1 })
    ).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Add channel' }))
    expect(screen.getByDisplayValue('channel-1')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }))
    await waitFor(() => expect(saveAiChannels).toHaveBeenCalledTimes(1))
    expect(JSON.stringify(saveAiChannels.mock.calls[0]?.[0])).not.toContain('secret')
  })
})
