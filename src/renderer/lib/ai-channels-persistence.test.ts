import { beforeEach, describe, expect, it, vi } from 'vitest'

const persistence = vi.hoisted(() => ({
  read: vi.fn(),
  write: vi.fn()
}))
const secure = vi.hoisted(() => ({
  setSecret: vi.fn(),
  getSecret: vi.fn(),
  deleteSecret: vi.fn()
}))
const tauri = vi.hoisted(() => ({ value: true }))

vi.mock('@/lib/api', () => ({
  persistenceApi: persistence,
  secureStorageApi: secure
}))
vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => tauri.value
}))

import {
  AI_CHANNELS_KEY,
  aiCredentialKey,
  emptyAiChannelsDocument,
  getAiChannelCredentialStatus,
  loadAiChannels,
  saveAiChannels,
  setAiChannelCredential
} from './ai-channels-persistence'

beforeEach(() => {
  vi.clearAllMocks()
  tauri.value = true
})

describe('AI channel persistence', () => {
  it('loads an empty versioned document when the key is absent', async () => {
    persistence.read.mockResolvedValue({ success: false, code: 'KEY_NOT_FOUND', error: 'missing' })
    await expect(loadAiChannels()).resolves.toEqual(emptyAiChannelsDocument())
    expect(persistence.read).toHaveBeenCalledWith(AI_CHANNELS_KEY)
  })

  it('round-trips only validated nonsecret channel config', async () => {
    const document = emptyAiChannelsDocument()
    persistence.write.mockResolvedValue({ success: true, data: undefined })
    await saveAiChannels(document)
    expect(persistence.write).toHaveBeenCalledWith(AI_CHANNELS_KEY, document)
  })

  it('never accepts a raw credential in a credential reference', async () => {
    expect(() => aiCredentialKey('bad id')).toThrow('AI channel id is invalid')
    await expect(setAiChannelCredential({ id: 'openai' }, '   ')).rejects.toThrow(
      'AI credential must not be empty'
    )
  })

  it('reports credential presence without returning the credential', async () => {
    secure.getSecret.mockResolvedValue({ success: true, data: 'secret-canary' })
    await expect(getAiChannelCredentialStatus('openai')).resolves.toEqual({
      success: true,
      data: { hasCredential: true }
    })
    const result = await getAiChannelCredentialStatus('openai')
    expect(JSON.stringify(result)).not.toContain('secret-canary')
  })

  it('does not attempt browser-local credential storage', async () => {
    tauri.value = false
    await expect(getAiChannelCredentialStatus('openai')).resolves.toMatchObject({
      success: false,
      code: 'AI_CREDENTIAL_STORAGE_UNAVAILABLE'
    })
    expect(secure.getSecret).not.toHaveBeenCalled()
  })
})
