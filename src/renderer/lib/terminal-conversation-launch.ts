/**
 * Create a Conversation whose backend is a terminal.
 *
 * Deliberately a separate path from `AgentLauncher.launch` rather than a branch
 * inside it. The two share exactly one thing — the execution target that picks
 * the folder. Everything else in the agent launch (config selection, models,
 * modes, prompt assembly, placeholder sessions, binding handoff) has no meaning
 * here, and threading a `backend` flag through it would leave every one of
 * those steps guarded by the same condition.
 *
 * The order is prepare -> spawn -> provision, and it matters:
 *
 * - `prepare` allocates the Conversation and creates its workspace directory,
 *   then stops. The Conversation sits in `allocating_workspace`.
 * - the terminal is spawned through the ordinary renderer path, so it gets a
 *   tab, the per-project limit, project env and worktree symlinks. Spawning it
 *   host-side inside `prepare` would have to reproduce all of that.
 * - `provision` records which terminal backs the Conversation and carries it to
 *   `ready`.
 *
 * A failure between the steps leaves the Conversation in `allocating_workspace`
 * with no backend declared, which the host's interrupted-creation sweep already
 * reconciles. That is why `prepare` does not report `ready` optimistically.
 */

import type { ExecutionTarget, ProjectAttachment } from '@shared/types/conversation.types'
import { runtimeT } from '@/i18n/runtime'
import { conversationApi } from '@/lib/conversation-api'
import { logFrontendError } from '@/lib/log-api'
import { spawnTerminalInPane } from '@/lib/terminal-spawn'

/** Mirrors the host's `PREPARE_CONVERSATION_SCHEMA_VERSION`. */
export const PREPARE_TERMINAL_CONVERSATION_SCHEMA_VERSION = 1

export interface LaunchTerminalConversationInput {
  paneId: string
  executionTarget: ExecutionTarget
  projectAttachment?: ProjectAttachment | null
  /** Project attribution for the terminal record; never the ownership key. */
  projectId?: string
  envVars?: Array<{ key: string; value: string; enabled?: boolean }>
  maxTerminalsPerProject?: number
}

export type LaunchTerminalConversationResult =
  | { success: true; conversationId: string; terminalId: string; error?: undefined }
  | { success: false; error: string; conversationId?: string; terminalId?: undefined }

function message(error: unknown, fallback: string): string {
  if (typeof error === 'string' && error.trim()) return error
  return error instanceof Error && error.message ? error.message : fallback
}

export async function launchTerminalConversation(
  input: LaunchTerminalConversationInput
): Promise<LaunchTerminalConversationResult> {
  const prepared = await conversationApi.prepareTerminalConversation({
    schemaVersion: PREPARE_TERMINAL_CONVERSATION_SCHEMA_VERSION,
    executionTarget: input.executionTarget,
    projectAttachment: input.projectAttachment ?? null
  })
  if (!prepared.success) {
    return {
      success: false,
      error: message(
        prepared.error,
        runtimeT('chat', 'terminalConversation.prepareFailed', 'Failed to create the conversation')
      )
    }
  }

  const { conversationId, executionCwd } = prepared.data
  const spawned = await spawnTerminalInPane(input.paneId, input.projectId ?? '', executionCwd, {
    conversationId,
    ...(input.envVars ? { envVars: input.envVars } : {}),
    ...(input.maxTerminalsPerProject !== undefined
      ? { maxTerminalsPerProject: input.maxTerminalsPerProject }
      : {})
  })
  if (!spawned.success) {
    // The Conversation stays in `allocating_workspace`. Reporting its id lets the
    // caller name what was left behind instead of silently orphaning it.
    void logFrontendError({
      source: 'terminal-conversation.spawn',
      message: `conversationId=${conversationId} spawn failed: ${spawned.error ?? 'unknown'}`
    })
    return {
      success: false,
      conversationId,
      error:
        spawned.error ?? runtimeT('terminal', 'errors.createFailed', 'Failed to create terminal')
    }
  }

  const provisioned = await conversationApi.provisionTerminalConversation(
    conversationId,
    spawned.terminalId
  )
  if (!provisioned.success) {
    // The terminal is real and already belongs to the Conversation; only the
    // durable "this is terminal-backed, and it is ready" record is missing.
    // Killing the terminal here would destroy work the user can see, so the
    // Conversation is left for the sweep and the failure is surfaced.
    void logFrontendError({
      source: 'terminal-conversation.provision',
      message: `conversationId=${conversationId} terminalId=${spawned.terminalId} provision failed: ${provisioned.error ?? 'unknown'}`
    })
    return {
      success: false,
      conversationId,
      error: message(
        provisioned.error,
        runtimeT(
          'chat',
          'terminalConversation.provisionFailed',
          'The terminal started but the conversation could not be finalized'
        )
      )
    }
  }

  return { success: true, conversationId, terminalId: spawned.terminalId }
}
