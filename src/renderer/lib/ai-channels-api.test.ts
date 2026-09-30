import { beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.hoisted(() => vi.fn())
const persistence = vi.hoisted(() => ({ read: vi.fn(), write: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('@/lib/api', () => ({ persistenceApi: persistence }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))

import { type AiChannelsDocument, aiChannelsApi } from './ai-channels-api'

describe('aiChannelsApi.load', () => {
  let stored: AiChannelsDocument | null

  beforeEach(() => {
    vi.clearAllMocks()
    stored = null
    invoke.mockImplementation(async (command: string, args?: { document: AiChannelsDocument }) => {
      if (command === 'ai_channels_get') {
        return { success: true, data: { document: stored, keys: stored ? { old: true } : {} } }
      }
      if (command === 'ai_channels_put') {
        stored = args?.document ?? null
        return { success: true, data: undefined }
      }
      throw new Error(`unexpected ${command}`)
    })
  })

  it('carries channels the user gave a key over once, keeping their ids', async () => {
    persistence.read.mockResolvedValue({
      success: true,
      data: {
        schemaVersion: 1,
        channels: [
          {
            id: 'old',
            displayName: 'Old relay',
            provider: 'openAiCompatible',
            baseUrl: 'https://relay.test/v1',
            enabled: true,
            modelIds: ['gpt-5'],
            credentialRef: { kind: 'keyring', ref: 'ai/channel/old', hasCredential: true }
          },
          // What the old page seeded on its own: never given a key.
          {
            id: 'channel-1',
            displayName: 'AI Channel 1',
            provider: 'openAiCompatible',
            baseUrl: 'https://api.openai.com/v1',
            enabled: true,
            modelIds: ['model'],
            credentialRef: { kind: 'keyring', ref: 'ai/channel/channel-1', hasCredential: false }
          },
          {
            id: 'no-url',
            displayName: 'Broken',
            provider: 'openRouter',
            credentialRef: { hasCredential: true }
          }
        ]
      }
    })
    const loaded = await aiChannelsApi.load()
    expect(loaded.success && loaded.data.document.channels).toEqual([
      {
        id: 'old',
        name: 'Old relay',
        api: 'openai-completions',
        baseUrl: 'https://relay.test/v1',
        enabled: true,
        models: ['gpt-5']
      }
    ])
    expect(loaded.success && loaded.data.keys).toEqual({ old: true })
    expect(invoke).toHaveBeenCalledWith('ai_channels_put', expect.anything())

    persistence.read.mockClear()
    await aiChannelsApi.load()
    expect(persistence.read).not.toHaveBeenCalled()
  })

  it('starts empty when there is nothing to carry over', async () => {
    persistence.read.mockResolvedValue({ success: false, code: 'KEY_NOT_FOUND' })
    const loaded = await aiChannelsApi.load()
    expect(loaded.success && loaded.data.document.channels).toEqual([])
    expect(invoke).not.toHaveBeenCalledWith('ai_channels_put', expect.anything())
  })
})
