import { describe, expect, it } from 'vitest'
import type { Project, ProjectGroup, Terminal } from '@/types/project'
import {
  availableScopes,
  groupTerminalsByProject,
  orderByRecency,
  scopeTerminals,
  type TerminalSwitcherContext
} from './terminal-switcher'

function terminal(id: string, projectId?: string): Terminal {
  return { id, name: id, projectId, shell: 'bash' }
}

const group: ProjectGroup = { id: 'g1', name: 'Group 1', projectIds: ['p1', 'p2'] }

const context: TerminalSwitcherContext = {
  terminals: [
    terminal('t1', 'p1'),
    terminal('t2', 'p2'),
    terminal('t3', 'p3'),
    terminal('t4') // unassigned
  ],
  activeProjectId: 'p1',
  activeGroup: group
}

describe('scopeTerminals', () => {
  it('should return only the active project at project scope', () => {
    expect(scopeTerminals('project', context).map((t) => t.id)).toEqual(['t1'])
  })

  it('should span every project in the group at group scope', () => {
    // This is the row the user gets by clicking a group in the sidebar. p3 is
    // outside the group and t4 has no project at all — neither may leak in.
    expect(scopeTerminals('group', context).map((t) => t.id)).toEqual(['t1', 't2'])
  })

  it('should include unassigned terminals at all scope', () => {
    expect(scopeTerminals('all', context).map((t) => t.id)).toEqual(['t1', 't2', 't3', 't4'])
  })

  it('should degrade group scope to the project list when no group is selected', () => {
    // Reachable when the selected group is deleted while the row is on group
    // scope. Falling back beats blanking the row.
    const orphaned = { ...context, activeGroup: null }
    expect(scopeTerminals('group', orphaned).map((t) => t.id)).toEqual(['t1'])
  })

  it('should return nothing at project scope when no project is active', () => {
    const noProject = { ...context, activeProjectId: null }
    expect(scopeTerminals('project', noProject)).toEqual([])
  })

  it('should return nothing for a group with no projects', () => {
    const emptyGroup: ProjectGroup = { ...group, projectIds: [] }
    expect(scopeTerminals('group', { ...context, activeGroup: emptyGroup })).toEqual([])
  })
})

describe('orderByRecency', () => {
  const all = [terminal('t1'), terminal('t2'), terminal('t3')]

  it('should put visited terminals first in visit order', () => {
    // Opening the switcher and pressing Enter must be the same jump as the
    // last-terminal shortcut, so the freshest visit has to lead.
    expect(orderByRecency(all, ['t3', 't1']).map((t) => t.id)).toEqual(['t3', 't1', 't2'])
  })

  it('should keep never-visited terminals in store order behind the visited ones', () => {
    expect(orderByRecency(all, ['t2']).map((t) => t.id)).toEqual(['t2', 't1', 't3'])
  })

  it('should ignore recent ids that are closed or out of scope', () => {
    // The MRU stack is global and keeps closed ids; a scoped list must not
    // grow entries back out of it.
    expect(orderByRecency([terminal('t1')], ['gone', 't1']).map((t) => t.id)).toEqual(['t1'])
  })

  it('should not repeat a terminal named twice in the stack', () => {
    expect(orderByRecency(all, ['t2', 't2']).map((t) => t.id)).toEqual(['t2', 't1', 't3'])
  })
})

describe('availableScopes', () => {
  it('should offer the group step only when a group is selected', () => {
    expect(availableScopes(context)).toEqual(['project', 'group', 'all'])
    expect(availableScopes({ ...context, activeGroup: null })).toEqual(['project', 'all'])
  })
})

describe('groupTerminalsByProject', () => {
  const projects: Project[] = [
    { id: 'p1', name: 'Alpha', color: 'blue' },
    { id: 'p2', name: 'Beta', color: 'green' }
  ]

  it('should collapse several terminals of one project into a single entry', () => {
    // The reported symptom: terminals named after their project rendered as
    // visually identical neighbouring chips.
    const entries = groupTerminalsByProject(
      [terminal('t1', 'p1'), terminal('t2', 'p1'), terminal('t3', 'p2')],
      projects,
      [],
      'No project'
    )

    expect(entries.map((entry) => entry.name)).toEqual(['Alpha', 'Beta'])
    expect(entries.map((entry) => entry.terminals.length)).toEqual([2, 1])
  })

  it('should open the most recently visited terminal of the project', () => {
    const entries = groupTerminalsByProject(
      [terminal('t1', 'p1'), terminal('t2', 'p1'), terminal('t3', 'p1')],
      projects,
      ['t3', 't1'],
      'No project'
    )

    // Not the first terminal in store order — returning to a project should
    // resume where the user left it.
    expect(entries[0].targetTerminalId).toBe('t3')
  })

  it('should fall back to store order when the project has no visited terminal', () => {
    const entries = groupTerminalsByProject(
      [terminal('t1', 'p1'), terminal('t2', 'p1')],
      projects,
      ['unrelated'],
      'No project'
    )

    expect(entries[0].targetTerminalId).toBe('t1')
  })

  it('should keep first-appearance order rather than recency order', () => {
    // A strip that reshuffles on every switch has to be re-read every time.
    const entries = groupTerminalsByProject(
      [terminal('t1', 'p1'), terminal('t2', 'p2')],
      projects,
      ['t2'],
      'No project'
    )

    expect(entries.map((entry) => entry.projectId)).toEqual(['p1', 'p2'])
  })

  it('should give unassigned terminals their own entry', () => {
    const entries = groupTerminalsByProject(
      [terminal('t1', 'p1'), terminal('t2')],
      projects,
      [],
      'No project'
    )

    expect(entries[1]).toMatchObject({ projectId: undefined, name: 'No project' })
  })

  it('should fall back to the project id when the project record is missing', () => {
    // A terminal can outlive its project record; the chip must still be usable.
    const entries = groupTerminalsByProject([terminal('t1', 'ghost')], projects, [], 'No project')

    expect(entries[0].name).toBe('ghost')
  })

  it('should report the worst status among the project terminals', () => {
    // A project with three live terminals and one crashed one is a project
    // with a problem; the chip has to say so.
    const live: Terminal = { ...terminal('t1', 'p1'), ptyId: 'pty-1' }
    const crashed: Terminal = { ...terminal('t2', 'p1'), ptyId: 'pty-2', healthStatus: 'crashed' }

    expect(groupTerminalsByProject([live, crashed], projects, [], '-')[0].status).toBe(
      'disconnected'
    )
  })

  it('should let attention outrank a disconnected sibling', () => {
    const crashed: Terminal = { ...terminal('t1', 'p1'), ptyId: 'pty-1', healthStatus: 'crashed' }
    const needy: Terminal = { ...terminal('t2', 'p1'), ptyId: 'pty-2', needsAttention: true }

    expect(groupTerminalsByProject([crashed, needy], projects, [], '-')[0].status).toBe('attention')
  })
})
