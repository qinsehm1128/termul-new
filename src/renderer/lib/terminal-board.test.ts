import { describe, expect, it } from 'vitest'
import {
  type Project,
  type ProjectGroup,
  sshTerminalProjectId,
  type Terminal
} from '@/types/project'
import {
  BOARD_WORKING_WINDOW_MS,
  boardStatusBucket,
  buildTerminalBoard,
  compareBoardRows,
  countBoardTerminals,
  filterTerminalBoard,
  latestActivity,
  terminalActivityText,
  terminalBoardStatus,
  terminalRowState
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

  it('lists quick and SSH terminals in their own groups, not as unassigned or by project', () => {
    // A quick terminal has no project and an SSH terminal carries a synthetic
    // `ssh-<profile>` id. Left off the board, the activity rail counted them
    // while the board did not, so the user could not see what was open.
    const board = buildTerminalBoard(
      [
        terminal('t-cost', { projectId: 'p-cost' }),
        terminal('t-quick', { quickTerminalId: 'q1', ptyId: 'pty-q' }),
        terminal('t-ssh', { projectId: sshTerminalProjectId('profile-1'), ptyId: 'pty-s' })
      ],
      projects,
      groups,
      new Map(),
      'Conversations',
      { quick: 'Quick', ssh: 'Remote' }
    )

    expect(board.map((group) => group.groupId)).toEqual(['g-ns', '__quick__', '__ssh__'])
    expect(board[1]).toMatchObject({ groupName: 'Quick', projects: [{ projectName: 'Quick' }] })
    expect(board[1].projects[0].terminals.map((item) => item.id)).toEqual(['t-quick'])
    expect(board[2]).toMatchObject({ groupName: 'Remote', projects: [{ projectName: 'Remote' }] })
    expect(board[2].projects[0].terminals.map((item) => item.id)).toEqual(['t-ssh'])
    expect(countBoardTerminals(board)).toBe(3)
  })

  it('lists a Conversation terminal under its Conversation, not its project', () => {
    // The board showed these inside the project they are attributed to, with
    // the Conversation's own sandbox as their cwd — so the project appeared to
    // own shells sitting in directories that have nothing to do with it.
    const board = buildTerminalBoard(
      [
        terminal('t-cost', { projectId: 'p-cost', name: 'cost-1' }),
        terminal('t-conv', { projectId: 'p-cost', name: 'conv-1', conversationId: 'conv-1' })
      ],
      projects,
      groups,
      new Map([['conv-1', 'Fix the parser']]),
      'Conversations'
    )

    expect(board.map((group) => group.groupId)).toEqual(['g-ns', '__conversations__'])
    expect(board[0].projects[0].terminals.map((item) => item.id)).toEqual(['t-cost'])
    expect(board[1].groupName).toBe('Conversations')
    expect(board[1].projects[0]).toMatchObject({
      conversationId: 'conv-1',
      projectName: 'Fix the parser'
    })
    expect(board[1].projects[0].terminals.map((item) => item.id)).toEqual(['t-conv'])
  })

  it('gives each Conversation its own block', () => {
    const board = buildTerminalBoard(
      [
        terminal('a', { projectId: 'p-cost', conversationId: 'conv-1' }),
        terminal('b', { projectId: 'p-other', conversationId: 'conv-2' })
      ],
      projects,
      groups,
      new Map([
        ['conv-1', 'First'],
        ['conv-2', 'Second']
      ]),
      'Conversations'
    )

    const conversationBlock = board.find((group) => group.groupId === '__conversations__')
    expect(conversationBlock?.projects.map((entry) => entry.projectName)).toEqual([
      'First',
      'Second'
    ])
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

  // `disconnected` means the transport or resume failed — something to act on.
  // A shell that ended with status 0 is not, and reusing the key showed those
  // terminals with the same alarming tone as a real failure.
  it('keeps a clean exit separate from a transport failure', () => {
    expect(terminalBoardStatus(terminal('e', { ptyId: 'pty', healthStatus: 'exited' }))).toBe(
      'exited'
    )
    expect(terminalBoardStatus(terminal('f', { ptyId: 'pty', healthStatus: 'crashed' }))).toBe(
      'disconnected'
    )
  })
})

describe('terminalRowState', () => {
  const now = 1_000_000
  const running = terminal('t', { ptyId: 'pty', healthStatus: 'running', viewState: 'visible' })

  it('calls a terminal busy only while it is still printing', () => {
    expect(terminalRowState(running, now - 500, now)).toBe('working')
    expect(terminalRowState(running, now - BOARD_WORKING_WINDOW_MS, now)).toBe('idle')
    expect(terminalRowState(running, undefined, now)).toBe('idle')
  })

  it('puts a blocked agent and a flagged terminal ahead of output', () => {
    expect(terminalRowState({ ...running, agentState: 'blocked' }, now, now)).toBe('attention')
    expect(terminalRowState({ ...running, needsAttention: true }, now, now)).toBe('attention')
  })

  it('reports an ended or cut-off process whatever it last printed', () => {
    expect(terminalRowState({ ...running, healthStatus: 'exited' }, now, now)).toBe('exited')
    expect(terminalRowState({ ...running, healthStatus: 'crashed' }, now, now)).toBe('disconnected')
  })
})

describe('boardStatusBucket', () => {
  it('files a cut-off terminal under "needs you" and an ended one under "ended"', () => {
    expect(boardStatusBucket('disconnected')).toBe('attention')
    expect(boardStatusBucket('attention')).toBe('attention')
    expect(boardStatusBucket('exited')).toBe('ended')
    expect(boardStatusBucket('working')).toBe('working')
  })
})

describe('compareBoardRows', () => {
  it('sorts the most urgent first, then the most recently active', () => {
    const rows = [
      { id: 'idle-old', state: 'idle' as const, lastActiveAt: 1 },
      { id: 'ended', state: 'exited' as const, lastActiveAt: 9 },
      { id: 'idle-new', state: 'idle' as const, lastActiveAt: 5 },
      { id: 'busy', state: 'working' as const, lastActiveAt: 2 },
      { id: 'needs', state: 'attention' as const, lastActiveAt: undefined }
    ]
    expect([...rows].sort(compareBoardRows).map((row) => row.id)).toEqual([
      'needs',
      'busy',
      'idle-new',
      'idle-old',
      'ended'
    ])
  })
})

describe('latestActivity', () => {
  it('takes the later of the output clock and the record stamp', () => {
    expect(latestActivity(terminal('t', { lastActivityTimestamp: 5 }), 9)).toBe(9)
    expect(latestActivity(terminal('t', { lastActivityTimestamp: 9 }), 5)).toBe(9)
    expect(latestActivity(terminal('t'), undefined)).toBeUndefined()
  })
})

describe('terminalActivityText', () => {
  it('prefers the title the program set, else the folder with home shortened', () => {
    expect(terminalActivityText(terminal('t', { oscTitle: '✳ Fix login', cwd: '/x' }))).toBe(
      '✳ Fix login'
    )
    expect(terminalActivityText(terminal('t', { cwd: '/Users/qs/project/app' }))).toBe(
      '~/project/app'
    )
    expect(terminalActivityText(terminal('t', { cwd: '/home/dev' }))).toBe('~')
    expect(terminalActivityText(terminal('t', { cwd: '/srv/app' }))).toBe('/srv/app')
  })
})
