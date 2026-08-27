import type { PersistedTerminalLayout } from '@shared/types/persistence.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Project } from '@/types/project'
import { countSnapshotTerminals, useLastSessionStore } from './last-session-store'

const loadPersistedTerminals = vi.fn()
vi.mock('@/hooks/useTerminalAutoSave', () => ({
  loadPersistedTerminals: (projectId: string) => loadPersistedTerminals(projectId)
}))

function project(id: string, name: string): Project {
  return { id, name, color: 'blue' }
}

function layout(names: string[], updatedAt = '2026-08-26T00:00:00.000Z'): PersistedTerminalLayout {
  return {
    activeTerminalId: null,
    updatedAt,
    terminals: names.map((name, index) => ({
      id: `${name}-${index}`,
      name,
      shell: 'bash'
    })) as PersistedTerminalLayout['terminals']
  }
}

describe('last-session-store', () => {
  beforeEach(() => {
    useLastSessionStore.getState().reset()
    loadPersistedTerminals.mockReset()
  })

  it('records how many terminals each project had', async () => {
    loadPersistedTerminals.mockImplementation(async (id: string) =>
      id === 'p1' ? layout(['build', 'claude']) : layout(['shell'])
    )

    await useLastSessionStore.getState().capture([project('p1', 'Alpha'), project('p2', 'Beta')])

    expect(useLastSessionStore.getState().snapshot?.projects).toEqual([
      { projectId: 'p1', name: 'Alpha', terminalCount: 2, terminalNames: ['build', 'claude'] },
      { projectId: 'p2', name: 'Beta', terminalCount: 1, terminalNames: ['shell'] }
    ])
  })

  it('omits projects that had no terminals', async () => {
    loadPersistedTerminals.mockImplementation(async (id: string) =>
      id === 'p1' ? layout(['build']) : layout([])
    )

    await useLastSessionStore.getState().capture([project('p1', 'Alpha'), project('p2', 'Beta')])

    expect(useLastSessionStore.getState().snapshot?.projects.map((p) => p.projectId)).toEqual([
      'p1'
    ])
  })

  it('tolerates a project with nothing persisted at all', async () => {
    loadPersistedTerminals.mockImplementation(async (id: string) =>
      id === 'p1' ? layout(['build']) : null
    )

    await useLastSessionStore.getState().capture([project('p1', 'Alpha'), project('p2', 'Beta')])

    expect(useLastSessionStore.getState().snapshot?.projects).toHaveLength(1)
  })

  it('reports the newest write across projects as the capture time', async () => {
    loadPersistedTerminals.mockImplementation(async (id: string) =>
      id === 'p1'
        ? layout(['a'], '2026-08-01T00:00:00.000Z')
        : layout(['b'], '2026-08-20T00:00:00.000Z')
    )

    await useLastSessionStore.getState().capture([project('p1', 'Alpha'), project('p2', 'Beta')])

    expect(useLastSessionStore.getState().snapshot?.capturedAt).toBe('2026-08-20T00:00:00.000Z')
  })

  it('never re-reads once captured', async () => {
    // The files stay live: the autosave rewrites them as soon as this session
    // opens a terminal. A second read would describe the current session and
    // silently overwrite the record of the one that was lost.
    loadPersistedTerminals.mockResolvedValue(layout(['build']))
    await useLastSessionStore.getState().capture([project('p1', 'Alpha')])

    loadPersistedTerminals.mockResolvedValue(layout(['something-new']))
    await useLastSessionStore.getState().capture([project('p1', 'Alpha')])

    expect(useLastSessionStore.getState().snapshot?.projects[0].terminalNames).toEqual(['build'])
  })

  it('shares one read between concurrent callers', async () => {
    loadPersistedTerminals.mockResolvedValue(layout(['build']))
    const projects = [project('p1', 'Alpha')]

    await Promise.all([
      useLastSessionStore.getState().capture(projects),
      useLastSessionStore.getState().capture(projects)
    ])

    expect(loadPersistedTerminals).toHaveBeenCalledTimes(1)
  })

  it('keeps the other projects when one cannot be read', async () => {
    // Fire-and-forget: a rejection here would surface as an unhandled promise
    // rejection instead of one missing row.
    loadPersistedTerminals.mockImplementation((id: string) => {
      // Synchronous throw on purpose: the loader is not guaranteed to hand back
      // a promise, so there may be nothing to attach a `.catch` to.
      if (id === 'p1') throw new Error('disk gone')
      return Promise.resolve(layout(['shell']))
    })

    await expect(
      useLastSessionStore.getState().capture([project('p1', 'Alpha'), project('p2', 'Beta')])
    ).resolves.toBeUndefined()

    expect(useLastSessionStore.getState().snapshot?.projects.map((p) => p.projectId)).toEqual([
      'p2'
    ])
  })

  it('counts terminals across the whole snapshot', () => {
    expect(
      countSnapshotTerminals({
        capturedAt: null,
        projects: [
          { projectId: 'p1', name: 'A', terminalCount: 2, terminalNames: [] },
          { projectId: 'p2', name: 'B', terminalCount: 3, terminalNames: [] }
        ]
      })
    ).toBe(5)
    expect(countSnapshotTerminals(null)).toBe(0)
  })
})
