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
 * This function stops at `prepare`, and that boundary is the fix for a bug that
 * shipped: it used to spawn the terminal here too. The pane it spawned into
 * belonged to whatever workspace was on screen at that instant, and opening the
 * new Conversation immediately replaced that workspace with the Conversation's
 * own — leaving a live terminal tab behind in the previous project and an empty
 * pane (which renders the launcher) in the Conversation the user just made.
 *
 * The terminal is opened by activation instead — see
 * `ensureConversationTerminal`. That is where the Conversation's workspace is
 * already loaded, and it makes first launch and every later reopen the same
 * path.
 *
 * A Conversation left between `prepare` and its first terminal sits in
 * `allocating_workspace` with no backend declared, which the host's
 * interrupted-creation sweep already reconciles.
 */

import type { ExecutionTarget, ProjectAttachment } from '@shared/types/conversation.types'
import { runtimeT } from '@/i18n/runtime'
import { conversationApi } from '@/lib/conversation-api'

/** Mirrors the host's `PREPARE_CONVERSATION_SCHEMA_VERSION`. */
export const PREPARE_TERMINAL_CONVERSATION_SCHEMA_VERSION = 1

export interface LaunchTerminalConversationInput {
  executionTarget: ExecutionTarget
  projectAttachment?: ProjectAttachment | null
}

export type LaunchTerminalConversationResult =
  | { success: true; conversationId: string; error?: undefined }
  | { success: false; error: string; conversationId?: undefined }

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
  return { success: true, conversationId: prepared.data.conversationId }
}
