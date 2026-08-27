import { isConversationId } from '@shared/types/conversation.types'
import type {
  ConversationComposerSnapshot,
  PersistedComposerOptions
} from '@shared/types/persistence.types'
import { PersistenceKeys } from '@shared/types/persistence.types'
import {
  overlayPendingLauncherOptions,
  type PendingLauncherOptions
} from '@/components/agents/pending-launcher-options'
import type { SessionConfigOption, SessionModelState, SessionModeState } from '@/lib/acp-api'
import { persistenceApi } from '@/lib/api'
import { logFrontendError } from '@/lib/log-api'

export type ComposerOptionsCache = {
  models?: SessionModelState | null
  modes?: SessionModeState | null
  configOptions?: SessionConfigOption[]
}

export type SessionComposerSource = {
  conversationId?: string
  models?: SessionModelState | null
  modes?: SessionModeState | null
  configOptions?: SessionConfigOption[]
}

const conversationComposerQueues = new Map<string, Promise<void>>()

export function conversationSnapshotToPending(
  snapshot: ConversationComposerSnapshot
): PendingLauncherOptions {
  return {
    modelId: snapshot.modelId,
    modeId: snapshot.modeId,
    configValues: snapshot.configValues ?? {}
  }
}

export function snapshotSessionComposer(
  session: SessionComposerSource,
  agentConfigId?: string
): ConversationComposerSnapshot {
  const currentModel = session.models?.availableModels.find(
    (model) => model.modelId === session.models?.currentModelId
  )
  const currentMode = session.modes?.availableModes.find(
    (mode) => mode.id === session.modes?.currentModeId
  )
  const configValues: Record<string, string> = {}
  const configLabels: Record<string, { optionName?: string; valueName?: string }> = {}
  for (const option of session.configOptions ?? []) {
    if (!option.currentValue) continue
    configValues[option.id] = option.currentValue
    const current = option.options.find((entry) => entry.value === option.currentValue)
    configLabels[option.id] = { optionName: option.name, valueName: current?.name }
  }
  const snapshot: ConversationComposerSnapshot = {}
  if (agentConfigId) snapshot.agentConfigId = agentConfigId
  if (session.models?.currentModelId) {
    snapshot.modelId = session.models.currentModelId
    snapshot.modelName = currentModel?.name
  }
  if (session.modes?.currentModeId) {
    snapshot.modeId = session.modes.currentModeId
    snapshot.modeName = currentMode?.name
  }
  if (Object.keys(configValues).length > 0) {
    snapshot.configValues = configValues
    snapshot.configLabels = configLabels
  }
  return snapshot
}

export function hasComposerSnapshot(
  snapshot: ConversationComposerSnapshot | null | undefined
): boolean {
  if (!snapshot) return false
  return Boolean(
    snapshot.agentConfigId ||
      snapshot.modelId ||
      snapshot.modeId ||
      (snapshot.configValues && Object.keys(snapshot.configValues).length > 0)
  )
}

export function sessionHasComposerControls(session: SessionComposerSource | undefined): boolean {
  if (!session) return false
  return Boolean(
    (session.models && session.models.availableModels.length > 0) ||
      (session.modes && session.modes.availableModes.length > 0) ||
      (session.configOptions && session.configOptions.length > 0)
  )
}

function synthesizeModels(snapshot: ConversationComposerSnapshot): SessionModelState | null {
  if (!snapshot.modelId) return null
  return {
    currentModelId: snapshot.modelId,
    availableModels: [{ modelId: snapshot.modelId, name: snapshot.modelName ?? snapshot.modelId }]
  }
}

function synthesizeModes(snapshot: ConversationComposerSnapshot): SessionModeState | null {
  if (!snapshot.modeId) return null
  return {
    currentModeId: snapshot.modeId,
    availableModes: [{ id: snapshot.modeId, name: snapshot.modeName ?? snapshot.modeId }]
  }
}

function synthesizeConfigOptions(snapshot: ConversationComposerSnapshot): SessionConfigOption[] {
  if (!snapshot.configValues) return []
  return Object.entries(snapshot.configValues).map(([id, valueId]) => ({
    id,
    name: snapshot.configLabels?.[id]?.optionName ?? id,
    type: 'select',
    currentValue: valueId,
    options: [{ value: valueId, name: snapshot.configLabels?.[id]?.valueName ?? valueId }]
  }))
}

export function hydrateComposerControls(
  snapshot: ConversationComposerSnapshot,
  cache?: ComposerOptionsCache | null
): {
  models: SessionModelState | null
  modes: SessionModeState | null
  configOptions: SessionConfigOption[]
} {
  const pending = conversationSnapshotToPending(snapshot)
  let models = cache?.models ?? synthesizeModels(snapshot)
  if (
    models &&
    snapshot.modelId &&
    !models.availableModels.some((model) => model.modelId === snapshot.modelId)
  ) {
    models = {
      ...models,
      availableModels: [
        ...models.availableModels,
        { modelId: snapshot.modelId, name: snapshot.modelName ?? snapshot.modelId }
      ]
    }
  }
  let modes = cache?.modes ?? synthesizeModes(snapshot)
  if (
    modes &&
    snapshot.modeId &&
    !modes.availableModes.some((mode) => mode.id === snapshot.modeId)
  ) {
    modes = {
      ...modes,
      availableModes: [
        ...modes.availableModes,
        { id: snapshot.modeId, name: snapshot.modeName ?? snapshot.modeId }
      ]
    }
  }
  const configOptions =
    (cache?.configOptions?.length ?? 0) > 0
      ? (cache?.configOptions ?? [])
      : synthesizeConfigOptions(snapshot)
  return overlayPendingLauncherOptions({
    models,
    modes,
    configOptions,
    pending
  })
}

export function persistConversationComposer(
  conversationId: string,
  snapshot: ConversationComposerSnapshot
): void {
  if (!isConversationId(conversationId) || !hasComposerSnapshot(snapshot)) return
  const key = PersistenceKeys.conversationComposer(conversationId)
  const prev = conversationComposerQueues.get(key) ?? Promise.resolve()
  const next = prev.then(async () => {
    const result = await persistenceApi.read<ConversationComposerSnapshot>(key)
    const existing = result.success ? (result.data ?? {}) : {}
    const merged: ConversationComposerSnapshot = { ...existing, ...snapshot }
    if (!snapshot.configValues && existing.configValues) merged.configValues = existing.configValues
    if (!snapshot.configLabels && existing.configLabels) merged.configLabels = existing.configLabels
    await persistenceApi.writeDebounced(key, merged)
  })
  conversationComposerQueues.set(key, next)
  next.catch((err) => {
    void logFrontendError({
      level: 'warn',
      source: 'conversation-composer.persist',
      message: `persist failed for ${conversationId}: ${err instanceof Error ? err.message : String(err)}`
    })
  })
  next.finally(() => {
    if (conversationComposerQueues.get(key) === next) conversationComposerQueues.delete(key)
  })
}

export async function readConversationComposerSnapshot(
  conversationId: string
): Promise<ConversationComposerSnapshot | null> {
  if (!isConversationId(conversationId)) return null
  const result = await persistenceApi.read<ConversationComposerSnapshot>(
    PersistenceKeys.conversationComposer(conversationId)
  )
  if (!result.success || !result.data || !hasComposerSnapshot(result.data)) return null
  return result.data
}

export async function readComposerSnapshotForSession(input: {
  conversationId?: string
  agentConfigId?: string
}): Promise<ConversationComposerSnapshot | null> {
  if (input.conversationId) {
    const conversation = await readConversationComposerSnapshot(input.conversationId)
    if (conversation) return conversation
  }
  if (!input.agentConfigId) return null
  const result = await persistenceApi.read<PersistedComposerOptions>(
    PersistenceKeys.lastComposerOptions(input.agentConfigId)
  )
  if (!result.success || !result.data) return null
  const fallback: ConversationComposerSnapshot = {
    agentConfigId: input.agentConfigId,
    modelId: result.data.modelId,
    modeId: result.data.modeId,
    configValues: result.data.configValues
  }
  return hasComposerSnapshot(fallback) ? fallback : null
}
