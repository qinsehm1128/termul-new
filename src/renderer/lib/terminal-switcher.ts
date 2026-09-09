import type { TerminalBoardStatusKey } from '@/lib/terminal-board'
import { terminalBoardStatus } from '@/lib/terminal-board'
import {
  isConversationScopedTerminal,
  isProjectScopedTerminal,
  type Project,
  type ProjectGroup,
  type Terminal
} from '@/types/project'

/**
 * Which terminals the switcher row offers. Deliberately three fixed steps
 * rather than a free filter: the row is a glance-and-click affordance, and an
 * unbounded horizontal strip stops being readable well before it stops
 * scrolling (Ghostty caps visible tabs around seven).
 */
export type TerminalSwitcherScope = 'project' | 'group' | 'all'

export interface TerminalSwitcherContext {
  terminals: readonly Terminal[]
  activeProjectId: string | null
  /** The group the user last clicked, or null when no group is selected. */
  activeGroup: ProjectGroup | null
}

/**
 * Terminals visible at a given scope, in store order.
 *
 * Total by construction: `group` with no active group degrades to the project
 * list rather than returning nothing, so a scope that is briefly unreachable
 * (group deleted while selected) can never blank the row.
 */
export function scopeTerminals(
  scope: TerminalSwitcherScope,
  context: TerminalSwitcherContext
): Terminal[] {
  const { terminals, activeProjectId, activeGroup } = context

  if (scope === 'all') return [...terminals]

  if (scope === 'group' && activeGroup) {
    // Widened so an unassigned terminal's `undefined` can be probed directly.
    // A `projectId !== undefined` pre-check would be dead code: projectIds is
    // string[], so undefined is never a member.
    const grouped = new Set<string | undefined>(activeGroup.projectIds)
    // Conversation terminals are excluded for the same reason as the project
    // scope below: they are attributed to a project but owned by a Conversation.
    return terminals.filter(
      (terminal) => grouped.has(terminal.projectId) && !isConversationScopedTerminal(terminal)
    )
  }

  if (!activeProjectId) return []
  return terminals.filter((terminal) => isProjectScopedTerminal(terminal, activeProjectId))
}

/**
 * Recency-first ordering for the quick switcher.
 *
 * Terminals the user has actually visited come first in visit order, then
 * everything else in store order. This is what makes the switcher useful with
 * no typing at all: the first entry is where you just were, so open-then-enter
 * is the same jump as the last-terminal shortcut.
 *
 * `recentIds` may name terminals that are closed or out of scope; they are
 * simply absent from the result rather than filtered beforehand.
 */
export function orderByRecency(
  terminals: readonly Terminal[],
  recentIds: readonly string[]
): Terminal[] {
  const byId = new Map(terminals.map((terminal) => [terminal.id, terminal]))
  const visited: Terminal[] = []
  for (const id of recentIds) {
    const terminal = byId.get(id)
    if (!terminal) continue
    visited.push(terminal)
    byId.delete(id)
  }
  return [...visited, ...byId.values()]
}

/**
 * Scopes worth offering. `group` is dropped when no group is selected — an
 * option that resolves to the same list as the one beside it is noise.
 */
export function availableScopes(context: TerminalSwitcherContext): TerminalSwitcherScope[] {
  return context.activeGroup ? ['project', 'group', 'all'] : ['project', 'all']
}

/**
 * Scopes for the bar, which shows one chip per project rather than one per
 * terminal. `project` is excluded on purpose: at project granularity it always
 * resolves to exactly one chip, so it is a step that cannot change what you
 * see. The overlay keeps all three because it is still terminal-granular.
 */
export type TerminalBarScope = Extract<TerminalSwitcherScope, 'group' | 'all'>

export const TERMINAL_BAR_SCOPES: readonly TerminalBarScope[] = ['group', 'all']

export interface SwitcherProjectEntry {
  /**
   * What this chip stands for.
   *
   * Conversation terminals get their own chip instead of joining the chip of
   * the project they are attributed to. They are a different kind of thing —
   * owned by a Conversation, not by the project — and folding them into the
   * project's count made the project claim terminals it does not own.
   */
  kind: 'project' | 'conversation'
  /** `undefined` for terminals belonging to no project; they still get a chip. */
  projectId: string | undefined
  /** Set on a `conversation` chip: where clicking it navigates. */
  conversationId?: string
  name: string
  terminals: Terminal[]
  /** Where a click lands — the most recently visited terminal of this chip. */
  targetTerminalId: string
  status: TerminalBoardStatusKey
}

/**
 * Worst-first, so a chip standing for several terminals reports the one that
 * needs the user rather than the one that happens to sort first. A project with
 * three live terminals and one crashed one is a project with a problem.
 */
const STATUS_PRECEDENCE: readonly TerminalBoardStatusKey[] = [
  'attention',
  'disconnected',
  'live',
  'hidden'
]

function aggregateStatus(terminals: readonly Terminal[]): TerminalBoardStatusKey {
  const present = new Set(terminals.map(terminalBoardStatus))
  return STATUS_PRECEDENCE.find((status) => present.has(status)) ?? 'hidden'
}

/**
 * Collapse terminals into one entry per project for the switcher bar.
 *
 * Projects keep first-appearance order rather than recency order: the bar is a
 * glance-and-click strip, and a row that reshuffles itself every time you
 * switch is a row you have to re-read every time. Recency decides which
 * terminal a chip opens, not where the chip sits.
 */
/** Bucket key: a project id, `undefined` for no project, or one Conversation. */
type BucketKey = string | undefined
type Bucket = { conversationId?: string; terminals: Terminal[] }

/**
 * One chip per Conversation, not one chip for all of them.
 *
 * A single shared chip could only ever open one Conversation — it showed a
 * count of two and left the second unreachable, because a chip opens exactly
 * one target. Conversations get the same treatment projects do: their own chip
 * each, named after the Conversation.
 */
export function groupTerminalsByProject(
  terminals: readonly Terminal[],
  projects: readonly Project[],
  recentIds: readonly string[],
  unassignedName: string,
  conversationNames: ReadonlyMap<string, string>
): SwitcherProjectEntry[] {
  const nameById = new Map(projects.map((project) => [project.id, project.name]))
  const order: string[] = []
  const buckets = new Map<string, Bucket>()

  for (const terminal of terminals) {
    // Conversation terminals never join a project bucket: they are attributed
    // to a project for labelling, but owned by their Conversation.
    const conversationId = isConversationScopedTerminal(terminal)
      ? terminal.conversationId
      : undefined
    const projectId: BucketKey = conversationId
      ? undefined
      : terminal.projectId?.trim() || undefined
    const key = conversationId ? `c:${conversationId}` : `p:${projectId ?? ''}`
    const bucket = buckets.get(key)
    if (bucket) {
      bucket.terminals.push(terminal)
      continue
    }
    buckets.set(key, { conversationId, terminals: [terminal] })
    order.push(key)
  }

  return order.map((key) => {
    const bucket = buckets.get(key) ?? { terminals: [] }
    // Non-empty by construction — a bucket only exists because a terminal
    // created it — so orderByRecency always yields a first entry.
    const target = orderByRecency(bucket.terminals, recentIds)[0]
    if (bucket.conversationId) {
      const conversationId = bucket.conversationId
      return {
        kind: 'conversation' as const,
        projectId: undefined,
        conversationId,
        name: conversationNames.get(conversationId) ?? conversationId.slice(0, 8),
        terminals: bucket.terminals,
        targetTerminalId: target.id,
        status: aggregateStatus(bucket.terminals)
      }
    }
    const projectId = key.slice(2) || undefined
    return {
      kind: 'project' as const,
      projectId,
      name: projectId ? (nameById.get(projectId) ?? projectId) : unassignedName,
      terminals: bucket.terminals,
      targetTerminalId: target.id,
      status: aggregateStatus(bucket.terminals)
    }
  })
}
