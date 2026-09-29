/**
 * AI channels: model APIs Se Manager itself calls. The document lives in the
 * desktop host (`ai-channels.json`); API keys live in the OS keychain and
 * never reach the renderer after they are entered.
 */

import type { IpcResult } from '@shared/types/ipc.types'
import { invoke } from '@tauri-apps/api/core'
import { persistenceApi } from '@/lib/api'
import { isTauriContext } from '@/lib/tauri-runtime'

/** Wire protocol, named as pi names them. */
export type AiApi = 'openai-completions' | 'openai-responses' | 'anthropic-messages'

export const AI_APIS: readonly AiApi[] = [
  'openai-completions',
  'openai-responses',
  'anthropic-messages'
]

export interface AiChannel {
  id: string
  name: string
  api: AiApi
  baseUrl: string
  enabled: boolean
  models: string[]
  headers?: Record<string, string>
}

export interface AiModelRef {
  channelId: string
  model: string
}

export interface AiChannelsDocument {
  schemaVersion: 2
  channels: AiChannel[]
  purposes: { mcpSummary?: AiModelRef }
}

export interface AiChannelsView {
  document: AiChannelsDocument
  /** Channel id → an API key is stored. */
  keys: Record<string, boolean>
}

export interface AiChannelTestResult {
  reply: string
  millis: number
}

export function emptyAiChannels(): AiChannelsDocument {
  return { schemaVersion: 2, channels: [], purposes: {} }
}

/** `[A-Za-z][A-Za-z0-9._-]{0,63}` — also the keychain key suffix. */
export function newChannelId(): string {
  return `ch-${Math.random().toString(36).slice(2, 10)}`
}

export type AiChannelPresetId = 'openai' | 'anthropic' | 'openrouter' | 'custom'

/** Starting points for a new channel. */
export const AI_CHANNEL_PRESETS: ReadonlyArray<{
  id: AiChannelPresetId
  name: string
  api: AiApi
  baseUrl: string
}> = [
  { id: 'openai', name: 'OpenAI', api: 'openai-responses', baseUrl: 'https://api.openai.com/v1' },
  {
    id: 'anthropic',
    name: 'Anthropic',
    api: 'anthropic-messages',
    baseUrl: 'https://api.anthropic.com'
  },
  {
    id: 'openrouter',
    name: 'OpenRouter',
    api: 'openai-completions',
    baseUrl: 'https://openrouter.ai/api/v1'
  },
  { id: 'custom', name: '', api: 'openai-completions', baseUrl: '' }
]

const LEGACY_KEY = 'ai/channels'

interface LegacyChannel {
  id?: unknown
  displayName?: unknown
  baseUrl?: unknown
  enabled?: unknown
  modelIds?: unknown
}

/**
 * The first document built from the old channel list, if there was one worth
 * keeping. Keys stay where they were: both versions use `ai/channel/<id>`.
 */
async function migrateLegacy(): Promise<AiChannelsDocument | null> {
  const legacy = await persistenceApi.read<{ channels?: LegacyChannel[] }>(LEGACY_KEY)
  if (!legacy.success || !Array.isArray(legacy.data?.channels)) return null
  const channels = legacy.data.channels.flatMap((entry): AiChannel[] => {
    if (typeof entry.id !== 'string' || typeof entry.baseUrl !== 'string') return []
    if (!/^[A-Za-z][A-Za-z0-9._-]{0,63}$/.test(entry.id) || entry.baseUrl.trim() === '') return []
    const models = Array.isArray(entry.modelIds)
      ? entry.modelIds.filter((model): model is string => typeof model === 'string')
      : []
    return [
      {
        id: entry.id,
        name: typeof entry.displayName === 'string' ? entry.displayName : entry.id,
        api: 'openai-completions',
        baseUrl: entry.baseUrl,
        enabled: entry.enabled !== false,
        models
      }
    ]
  })
  return channels.length > 0 ? { ...emptyAiChannels(), channels } : null
}

const DESKTOP_ONLY: IpcResult<never> = {
  success: false,
  error: 'AI channels are managed by the desktop app',
  code: 'DESKTOP_ONLY'
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<IpcResult<T>> {
  if (!isTauriContext()) return DESKTOP_ONLY
  try {
    return await invoke<IpcResult<T>>(command, args)
  } catch (error) {
    return {
      success: false,
      error: error instanceof Error ? error.message : String(error),
      code: 'INVOKE_ERROR'
    }
  }
}

export const aiChannelsApi = {
  async load(): Promise<IpcResult<AiChannelsView>> {
    const result = await call<{
      document: AiChannelsDocument | null
      keys: Record<string, boolean>
    }>('ai_channels_get')
    if (!result.success) return result
    if (result.data.document) {
      return { success: true, data: { document: result.data.document, keys: result.data.keys } }
    }
    const migrated = await migrateLegacy()
    if (!migrated) return { success: true, data: { document: emptyAiChannels(), keys: {} } }
    const saved = await call<void>('ai_channels_put', { document: migrated })
    if (!saved.success) return saved
    // Reread so the stored-key flags of the migrated channels are known.
    return aiChannelsApi.load()
  },
  save: (document: AiChannelsDocument) => call<void>('ai_channels_put', { document }),
  setKey: (id: string, key: string | null) => call<void>('ai_channel_set_key', { id, key }),
  models: (channel: AiChannel) => call<string[]>('ai_channel_models', { channel }),
  test: (channel: AiChannel, model: string) =>
    call<AiChannelTestResult>('ai_channel_test', { channel, model })
}
