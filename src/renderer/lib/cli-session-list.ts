import type { CliSessionAgentId, DiscoveredCliSession } from '@shared/types/cli-session.types'
import { CLI_SESSION_AGENT_IDS } from '@shared/types/cli-session.types'
import { pathBasename } from '@/components/lists/path-basename'

export type CliSessionSort = 'updated' | 'created'
export type CliSessionGroup = 'agent' | 'folder'

export function cliSessionTimestamp(session: DiscoveredCliSession, sort: CliSessionSort): number {
  const raw = sort === 'created' ? session.createdAt : (session.updatedAt ?? session.createdAt)
  const parsed = raw ? Date.parse(raw) : Number.NaN
  return Number.isNaN(parsed) ? 0 : parsed
}

export function sortCliSessions(
  sessions: DiscoveredCliSession[],
  sort: CliSessionSort
): DiscoveredCliSession[] {
  return [...sessions].sort((left, right) => {
    const delta = cliSessionTimestamp(right, sort) - cliSessionTimestamp(left, sort)
    if (delta !== 0) return delta
    return left.id.localeCompare(right.id)
  })
}

export function filterCliSessions(
  sessions: DiscoveredCliSession[],
  query: string,
  hideEmpty: boolean,
  agentIds?: ReadonlySet<CliSessionAgentId>
): DiscoveredCliSession[] {
  const needle = query.trim().toLowerCase()
  return sessions.filter((session) => {
    if (agentIds && !agentIds.has(session.agentId)) return false
    if (hideEmpty && session.messageCount === 0) return false
    if (!needle) return true
    const haystack = [
      session.title,
      session.sessionId,
      session.cwd ?? '',
      session.filePath,
      session.agentId
    ]
      .join(' ')
      .toLowerCase()
    return haystack.includes(needle)
  })
}

export function groupCliSessions(
  sessions: DiscoveredCliSession[],
  group: CliSessionGroup
): Array<{ key: string; label: string; sessions: DiscoveredCliSession[] }> {
  if (group === 'folder') {
    const folders = new Map<string, DiscoveredCliSession[]>()
    for (const session of sessions) {
      const key = session.cwd ?? ''
      const list = folders.get(key) ?? []
      list.push(session)
      folders.set(key, list)
    }
    return [...folders.entries()]
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, grouped]) => ({
        key: key || 'unknown-cwd',
        label: pathBasename(key) || key || '—',
        sessions: grouped
      }))
  }

  const agents = new Map<CliSessionAgentId, DiscoveredCliSession[]>()
  for (const session of sessions) {
    const list = agents.get(session.agentId) ?? []
    list.push(session)
    agents.set(session.agentId, list)
  }
  return CLI_SESSION_AGENT_IDS.filter((agentId) => agents.has(agentId)).map((agentId) => ({
    key: agentId,
    label: agentId,
    sessions: agents.get(agentId) ?? []
  }))
}
