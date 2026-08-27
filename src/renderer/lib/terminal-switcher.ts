import type { TerminalBoardStatusKey } from '@/lib/terminal-board'
import { terminalBoardStatus } from '@/lib/terminal-board'
import type { Project, ProjectGroup, Terminal } from '@/types/project'

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
    return terminals.filter((terminal) => grouped.has(terminal.projectId))
  }

  if (!activeProjectId) return []
  return terminals.filter((terminal) => terminal.projectId === activeProjectId)
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
  /** `undefined` for terminals belonging to no project; they still get a chip. */
  projectId: string | undefined
  name: string
  terminals: Terminal[]
  /** Where a click lands — the most recently visited terminal of this project. */
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
export function groupTerminalsByProject(
  terminals: readonly Terminal[],
  projects: readonly Project[],
  recentIds: readonly string[],
  unassignedName: string
): SwitcherProjectEntry[] {
  const nameById = new Map(projects.map((project) => [project.id, project.name]))
  const order: (string | undefined)[] = []
  const byProject = new Map<string | undefined, Terminal[]>()

  for (const terminal of terminals) {
    const projectId = terminal.projectId?.trim() || undefined
    const bucket = byProject.get(projectId)
    if (bucket) {
      bucket.push(terminal)
      continue
    }
    byProject.set(projectId, [terminal])
    order.push(projectId)
  }

  return order.map((projectId) => {
    const owned = byProject.get(projectId) ?? []
    return {
      projectId,
      name: projectId ? (nameById.get(projectId) ?? projectId) : unassignedName,
      terminals: owned,
      // `owned` is non-empty by construction — a bucket only exists because a
      // terminal created it — so orderByRecency always yields a first entry.
      targetTerminalId: orderByRecency(owned, recentIds)[0].id,
      status: aggregateStatus(owned)
    }
  })
}
