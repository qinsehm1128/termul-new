/**
 * Pure decision for how to reopen a persisted chat session (ADR-003.7).
 *
 * - 'resume' → agent connected and advertises sessionCapabilities.resume:
 *              restore the live context without replaying history already
 *              owned by Se.
 * - 'load'   → resume is unavailable and the agent advertises `loadSession`:
 *              call session/load and accept its session/update replay.
 * - 'local'  → no connected agent or no capability: show the locally persisted
 *              transcript (read-only history).
 *
 * Agent-native discovered sessions have no Se transcript, so they prefer
 * load when available; resume remains their capability fallback.
 *
 * A gated command (load/resume) MUST NOT be attempted unless its capability is
 * present, so the decision encodes the capability check.
 */
import type { AgentCapabilities } from '@/lib/acp-api'

export type ResumeStrategy = 'load' | 'resume' | 'local'

export interface ResumeInput {
  connected: boolean
  capabilities: AgentCapabilities | null
  /** Se can render the transcript without asking the agent to replay it. */
  localHistoryAvailable: boolean
}

export function decideResume({
  connected,
  capabilities,
  localHistoryAvailable
}: ResumeInput): ResumeStrategy {
  if (!connected || !capabilities) return 'local'
  const resume = capabilities.sessionCapabilities?.resume
  const canResume = resume !== undefined && resume !== null
  if (localHistoryAvailable && canResume) return 'resume'
  if (capabilities.loadSession === true) return 'load'
  if (canResume) return 'resume'
  return 'local'
}
