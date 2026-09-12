import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useGitStatusStore } from '@/stores/git-status-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import type { PaneNode } from '@/types/workspace.types'
import {
  collectOpenGitTabCwds,
  isPathWithinRepo,
  resetGitStatusRefreshSchedulerForTests,
  scheduleGitStatusRefreshForPath
} from './schedule-git-status-refresh'

vi.mock('@/stores/git-status-store', () => ({
  useGitStatusStore: {
    getState: vi.fn()
  }
}))

const REPO = 'C:/project'
const refreshStatus = vi.fn().mockResolvedValue(undefined)

function leafWithGitTab(cwd: string): PaneNode {
  return {
    type: 'leaf',
    id: 'leaf-1',
    tabs: [{ type: 'git', id: 'git-1', cwd }],
    activeTabId: 'git-1'
  }
}

beforeEach(() => {
  vi.useFakeTimers()
  resetGitStatusRefreshSchedulerForTests()
  refreshStatus.mockClear()
  vi.mocked(useGitStatusStore.getState).mockReturnValue({
    refreshStatus
  } as unknown as ReturnType<typeof useGitStatusStore.getState>)
  useWorkspaceStore.setState({ root: leafWithGitTab(REPO) })
})

afterEach(() => {
  vi.useRealTimers()
  resetGitStatusRefreshSchedulerForTests()
})

describe('isPathWithinRepo', () => {
  it('matches file under repo with mixed separators', () => {
    expect(isPathWithinRepo('C:\\project\\src\\a.ts', REPO)).toBe(true)
  })

  it('rejects paths outside repo', () => {
    expect(isPathWithinRepo('C:/other/file.ts', REPO)).toBe(false)
  })

  it('matches Windows paths case-insensitively', () => {
    expect(isPathWithinRepo('c:/project/src/a.ts', 'C:/project')).toBe(true)
  })
})

describe('collectOpenGitTabCwds', () => {
  it('returns cwd from open git tabs only', () => {
    expect(collectOpenGitTabCwds()).toEqual([REPO])
  })
})

describe('scheduleGitStatusRefreshForPath', () => {
  it('debounces refreshStatus for matching open git tab', async () => {
    scheduleGitStatusRefreshForPath(`${REPO}/src/foo.ts`)
    scheduleGitStatusRefreshForPath(`${REPO}/src/bar.ts`)

    expect(refreshStatus).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1000)

    expect(refreshStatus).toHaveBeenCalledTimes(1)
    expect(refreshStatus).toHaveBeenCalledWith(REPO)
  })

  it('does not reset debounce for unrelated paths while a refresh is pending', async () => {
    scheduleGitStatusRefreshForPath(`${REPO}/src/foo.ts`)
    await vi.advanceTimersByTimeAsync(200)
    scheduleGitStatusRefreshForPath('C:/other/outside.ts')
    await vi.advanceTimersByTimeAsync(200)

    expect(refreshStatus).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(850)

    expect(refreshStatus).toHaveBeenCalledTimes(1)
    expect(refreshStatus).toHaveBeenCalledWith(REPO)
  })

  it('refreshes only repos that match the changed path', async () => {
    const otherRepo = 'C:/other-project'
    useWorkspaceStore.setState({
      root: {
        type: 'split',
        id: 'split-1',
        direction: 'horizontal',
        children: [leafWithGitTab(REPO), leafWithGitTab(otherRepo)],
        sizes: [50, 50]
      }
    })

    scheduleGitStatusRefreshForPath(`${REPO}/src/foo.ts`)
    await vi.advanceTimersByTimeAsync(1000)

    expect(refreshStatus).toHaveBeenCalledTimes(1)
    expect(refreshStatus).toHaveBeenCalledWith(REPO)
  })

  // Recursive root watching delivers up to 500 changes in one synchronous
  // dispatch loop. Collecting the open Git tabs walks the whole pane tree, so
  // doing it per event meant walking that tree hundreds of times for a set that
  // cannot change between two events of the same batch.
  it('walks the pane tree once per synchronous batch, then again on the next', async () => {
    const getState = vi.spyOn(useWorkspaceStore, 'getState')

    scheduleGitStatusRefreshForPath(`${REPO}/a.ts`)
    scheduleGitStatusRefreshForPath(`${REPO}/b.ts`)
    scheduleGitStatusRefreshForPath(`${REPO}/c.ts`)
    expect(getState).toHaveBeenCalledTimes(1)

    // A new batch is a new microtask, and a Git tab may have opened since.
    await Promise.resolve()
    scheduleGitStatusRefreshForPath(`${REPO}/d.ts`)
    expect(getState).toHaveBeenCalledTimes(2)

    getState.mockRestore()
  })

  it('no-ops when no git tab is open', async () => {
    useWorkspaceStore.setState({
      root: {
        type: 'leaf',
        id: 'leaf-1',
        tabs: [{ type: 'editor', id: 'e-1', filePath: `${REPO}/a.ts` }],
        activeTabId: 'e-1'
      }
    })

    scheduleGitStatusRefreshForPath(`${REPO}/src/foo.ts`)
    await vi.advanceTimersByTimeAsync(1000)

    expect(refreshStatus).not.toHaveBeenCalled()
  })
})
