import { canonicalizeClaudeModelId } from '@/components/chat/chat-input-bar-config'
import type { SessionConfigOption, SessionModelState, SessionModeState } from '@/lib/acp-api'

/** Launcher selections made against cached options before a live session exists. */
export type PendingLauncherOptions = {
  modelId?: string
  modeId?: string
  configValues: Record<string, string>
}

export function emptyPendingLauncherOptions(): PendingLauncherOptions {
  return { configValues: {} }
}

export function hasPendingLauncherOptions(pending: PendingLauncherOptions): boolean {
  return Boolean(pending.modelId || pending.modeId || Object.keys(pending.configValues).length > 0)
}

/** Paint pending selections on top of live or cached option state. */
export function overlayPendingLauncherOptions(input: {
  models: SessionModelState | null | undefined
  modes: SessionModeState | null | undefined
  configOptions: SessionConfigOption[]
  pending: PendingLauncherOptions
}): {
  models: SessionModelState | null
  modes: SessionModeState | null
  configOptions: SessionConfigOption[]
} {
  const { pending } = input
  const pendingModelId = pending.modelId ? canonicalizeClaudeModelId(pending.modelId) : undefined
  const models =
    input.models == null
      ? null
      : pendingModelId
        ? { ...input.models, currentModelId: pendingModelId }
        : input.models
  const modes =
    input.modes == null
      ? null
      : pending.modeId
        ? { ...input.modes, currentModeId: pending.modeId }
        : input.modes
  const configOptions =
    Object.keys(pending.configValues).length === 0
      ? input.configOptions
      : input.configOptions.map((option) => {
          const next = pending.configValues[option.id]
          if (next == null) return option
          return {
            ...option,
            currentValue: option.category === 'model' ? canonicalizeClaudeModelId(next) : next
          }
        })
  return { models, modes, configOptions }
}
