import type { Project, ProjectColor, ProjectGroup, Terminal } from '@/types/project'
import { isOpenTerminalView } from '@/types/project'

export interface TerminalBoardProjectBlock {
  projectId: string
  projectName: string
  color: ProjectColor
  archived: boolean
  terminals: Terminal[]
}

export interface TerminalBoardGroupBlock {
  groupId: string | null
  groupName: string
  color?: ProjectColor
  projects: TerminalBoardProjectBlock[]
}

export type TerminalBoardStatusKey = 'live' | 'hidden' | 'exited' | 'disconnected' | 'attention'

export function terminalBoardStatus(terminal: Terminal): TerminalBoardStatusKey {
  if (terminal.needsAttention) return 'attention'
  // `exited` is its own key: `disconnected` means the transport or resume
  // failed, which is a problem to act on. A shell that ended with status 0 is
  // not, and must not be shown with the same alarming tone.
  if (terminal.healthStatus === 'exited') return 'exited'
  if (terminal.healthStatus === 'disconnected' || terminal.healthStatus === 'crashed') {
    return 'disconnected'
  }
  if (terminal.ptyId && !isOpenTerminalView(terminal)) return 'hidden'
  if (terminal.ptyId) return 'live'
  return 'disconnected'
}

function projectGroupId(projectId: string, groups: readonly ProjectGroup[]): string | undefined {
  return groups.find((group) => group.projectIds.includes(projectId))?.id
}

export function buildTerminalBoard(
  terminals: readonly Terminal[],
  projects: readonly Project[],
  groups: readonly ProjectGroup[]
): TerminalBoardGroupBlock[] {
  const projectById = new Map(projects.map((project) => [project.id, project]))
  const usedProjectIds = new Set<string>()
  const unassigned: Terminal[] = []
  const byProject = new Map<string, Terminal[]>()

  for (const terminal of terminals) {
    const projectId = terminal.projectId?.trim()
    if (!projectId) {
      unassigned.push(terminal)
      continue
    }
    const list = byProject.get(projectId) ?? []
    list.push(terminal)
    byProject.set(projectId, list)
    usedProjectIds.add(projectId)
  }

  const toProjectBlock = (projectId: string): TerminalBoardProjectBlock => {
    const project = projectById.get(projectId)
    return {
      projectId,
      projectName: project?.name || projectId,
      color: project?.color ?? 'gray',
      archived: Boolean(project?.isArchived),
      terminals: byProject.get(projectId) ?? []
    }
  }

  const blocks: TerminalBoardGroupBlock[] = []

  for (const group of groups) {
    const groupedProjects = group.projectIds
      .filter((projectId) => usedProjectIds.has(projectId))
      .map(toProjectBlock)
    if (groupedProjects.length === 0) continue
    blocks.push({
      groupId: group.id,
      groupName: group.name,
      color: group.color,
      projects: groupedProjects
    })
  }

  const ungroupedProjects = [...usedProjectIds]
    .filter((projectId) => !projectGroupId(projectId, groups))
    .map(toProjectBlock)
  if (ungroupedProjects.length > 0) {
    blocks.push({
      groupId: null,
      groupName: '',
      projects: ungroupedProjects
    })
  }

  if (unassigned.length > 0) {
    blocks.push({
      groupId: '__unassigned__',
      groupName: '',
      projects: [
        {
          projectId: '',
          projectName: '',
          color: 'gray',
          archived: false,
          terminals: unassigned
        }
      ]
    })
  }

  return blocks
}

export function filterTerminalBoard(
  board: readonly TerminalBoardGroupBlock[],
  query: string
): TerminalBoardGroupBlock[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return [...board]

  const matchesTerminal = (terminal: Terminal): boolean => {
    const haystack = [
      terminal.name,
      terminal.shell,
      terminal.cwd,
      terminal.agentName,
      terminal.kind
    ]
      .filter(Boolean)
      .join(' ')
      .toLowerCase()
    return haystack.includes(needle)
  }

  return board
    .map((group) => {
      const groupMatches = group.groupName.toLowerCase().includes(needle)
      const projects = group.projects
        .map((project) => {
          if (groupMatches || project.projectName.toLowerCase().includes(needle)) {
            return project
          }
          const terminals = project.terminals.filter(matchesTerminal)
          return terminals.length > 0 ? { ...project, terminals } : null
        })
        .filter((project): project is TerminalBoardProjectBlock => project !== null)
      return projects.length > 0 ? { ...group, projects } : null
    })
    .filter((group): group is TerminalBoardGroupBlock => group !== null)
}

export function countBoardTerminals(board: readonly TerminalBoardGroupBlock[]): number {
  return board.reduce(
    (total, group) =>
      total + group.projects.reduce((sum, project) => sum + project.terminals.length, 0),
    0
  )
}
