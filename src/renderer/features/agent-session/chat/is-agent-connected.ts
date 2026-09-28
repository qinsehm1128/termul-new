import type { AgentStatus } from '@/features/agent-session/stores/acp-store'

export function isAgentConnected(
  session: { status: string } | null | undefined,
  agentStatus: AgentStatus | undefined
): boolean {
  return session != null && session.status !== 'closed' && agentStatus === 'connected'
}
