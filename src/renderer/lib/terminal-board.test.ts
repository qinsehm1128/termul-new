import { describe, expect, it } from 'vitest'
import type { Project, ProjectGroup, Terminal } from '@/types/project'
import {
  buildTerminalBoard,
  countBoardTerminals,
  filterTerminalBoard,
  terminalBoardStatus
} from './terminal-board'

function project(id: string, name: string, extras: Partial<Project> = {}): Project {
  return { id, name, color: 'blue', path: `/p/${id}`, ...extras }
}

function terminal(id: string, extras: Partial<Terminal> = {}): Terminal {
  return {
    id,
    name: extras.name ?? id,
    projectId: extras.projectId,
    shell: extras.shell ?? 'zsh',
    cwd: extras.cwd ?? '/tmp',
    ...extras
  }
}

const groups: ProjectGroup[] = [
  { id: 'g-ns', name: 'Acme', projectIds: ['p-cost', 'p-other'], isCollapsed: false }
]

const projects: Project[] = [
  project('p-cost', 'logistics-api'),
  project('p-other', 'other'),
  project('p-loose', 'loose')
]

describe('buildTerminalBoard', () => {
  it('nests terminals under group then project, and keeps ungrouped / unassigned', () => {
    const board = buildTerminalBoard(
      [
        terminal('t-cost', { projectId: 'p-cost', name: 'cost-1', ptyId: 'pty-1' }),
        terminal('t-loose', { projectId: 'p-loose', name: 'loose-1' }),
        terminal('t-chat', { name: 'chat-only' })
      ],
      projects,
      groups
    )

    expect(board.map((group) => group.groupId)).toEqual(['g-ns', null, '__unassigned__'])
    expect(board[0].projects[0]).toMatchObject({
      projectId: 'p-cost',
      projectName: 'logistics-api'
    })
    expect(board[0].projects[0].terminals.map((item) => item.id)).toEqual(['t-cost'])
    expect(board[1].projects[0].projectId).toBe('p-loose')
    expect(board[2].projects[0].terminals.map((item) => item.id)).toEqual(['t-chat'])
    expect(countBoardTerminals(board)).toBe(3)
  })

  it('omits empty groups', () => {
    const board = buildTerminalBoard(
      [terminal('t-loose', { projectId: 'p-loose' })],
      projects,
      groups
    )
    expect(board).toHaveLength(1)
    expect(board[0].groupId).toBeNull()
  })
})

describe('filterTerminalBoard', () => {
  const board = buildTerminalBoard(
    [
      terminal('t-cost', { projectId: 'p-cost', name: 'deploy', cwd: '/srv/logistics' }),
      terminal('t-loose', { projectId: 'p-loose', name: 'logs' })
    ],
    projects,
    groups
  )

  it('keeps a whole project when the group name matches', () => {
    const filtered = filterTerminalBoard(board, 'acme')
    expect(countBoardTerminals(filtered)).toBe(1)
    expect(filtered[0].projects[0].projectId).toBe('p-cost')
  })

  it('filters to matching terminal names', () => {
    const filtered = filterTerminalBoard(board, 'logs')
    expect(countBoardTerminals(filtered)).toBe(1)
    expect(filtered[0].projects[0].terminals[0].id).toBe('t-loose')
  })
})

describe('terminalBoardStatus', () => {
  it('classifies live, hidden, and attention terminals', () => {
    expect(terminalBoardStatus(terminal('a', { ptyId: 'pty', healthStatus: 'running' }))).toBe(
      'live'
    )
    expect(
      terminalBoardStatus(terminal('b', { ptyId: 'pty', viewState: 'hidden', isHidden: true }))
    ).toBe('hidden')
    expect(terminalBoardStatus(terminal('c', { needsAttention: true, ptyId: 'pty' }))).toBe(
      'attention'
    )
    expect(terminalBoardStatus(terminal('d', { healthStatus: 'disconnected' }))).toBe(
      'disconnected'
    )
  })
})
