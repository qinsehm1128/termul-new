import type { AiChannel, AiChannelsDocument } from '@shared/types/ai-channels.types'
import {
  AI_CHANNELS_SCHEMA_VERSION,
  parseAiChannelsDocument,
  serializeAiChannelsDocument
} from '@shared/types/ai-channels.types'
import type { IpcResult } from '@shared/types/ipc.types'
import { persistenceApi, secureStorageApi } from '@/lib/api'
import { isTauriContext } from '@/lib/tauri-runtime'

export const AI_CHANNELS_KEY = 'ai/channels'
export const AI_CREDENTIAL_KEY_PREFIX = 'ai/channel/'

export function emptyAiChannelsDocument(): AiChannelsDocument {
  return {
    schemaVersion: AI_CHANNELS_SCHEMA_VERSION,
    revision: 1,
    channels: [],
    profiles: [],
    routes: []
  }
}

export async function loadAiChannels(): Promise<AiChannelsDocument> {
  const result = await persistenceApi.read<unknown>(AI_CHANNELS_KEY)
  if (!result.success) {
    if (result.code === 'KEY_NOT_FOUND') return emptyAiChannelsDocument()
    throw new Error(result.error)
  }
  return parseAiChannelsDocument(result.data)
}

export async function saveAiChannels(document: AiChannelsDocument): Promise<void> {
  const normalized = parseAiChannelsDocument(serializeAiChannelsDocument(document))
  const result = await persistenceApi.write(AI_CHANNELS_KEY, normalized)
  if (!result.success) throw new Error(result.error)
}

export function aiCredentialKey(channelId: string): string {
  if (!/^[A-Za-z][A-Za-z0-9._-]{0,63}$/.test(channelId)) {
    throw new TypeError('AI channel id is invalid')
  }
  return `${AI_CREDENTIAL_KEY_PREFIX}${channelId}`
}

export async function setAiChannelCredential(
  channel: Pick<AiChannel, 'id'>,
  secret: string
): Promise<void> {
  if (!isTauriContext()) {
    throw new Error('AI_CREDENTIAL_STORAGE_UNAVAILABLE')
  }
  if (secret.trim() === '') throw new TypeError('AI credential must not be empty')
  const result = await secureStorageApi.setSecret(aiCredentialKey(channel.id), secret)
  if (!result.success) throw new Error(result.code || 'AI_CREDENTIAL_STORAGE_UNAVAILABLE')
}

export async function deleteAiChannelCredential(channelId: string): Promise<void> {
  if (!isTauriContext()) {
    throw new Error('AI_CREDENTIAL_STORAGE_UNAVAILABLE')
  }
  const result = await secureStorageApi.deleteSecret(aiCredentialKey(channelId))
  if (!result.success) throw new Error(result.code || 'AI_CREDENTIAL_STORAGE_UNAVAILABLE')
}

export async function getAiChannelCredentialStatus(
  channelId: string
): Promise<IpcResult<{ hasCredential: boolean }>> {
  if (!isTauriContext()) {
    return {
      success: false,
      error: 'AI_CREDENTIAL_STORAGE_UNAVAILABLE',
      code: 'AI_CREDENTIAL_STORAGE_UNAVAILABLE'
    }
  }
  const result = await secureStorageApi.getSecret(aiCredentialKey(channelId))
  if (!result.success) {
    if (result.code === 'KEY_NOT_FOUND') return { success: true, data: { hasCredential: false } }
    return { success: false, error: result.code, code: result.code }
  }
  return { success: true, data: { hasCredential: result.data.length > 0 } }
}
