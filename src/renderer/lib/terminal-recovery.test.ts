import type { TerminalStatus } from '@shared/types/ipc.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({ list: vi.fn(), layout: vi.fn(), restoring: vi.fn() }))
vi.mock('@/lib/api', () => ({ terminalApi: { list: mocks.list } }))
vi.mock('@/hooks/useTerminalAutoSave', () => ({
  loadPersistedTerminals: mocks.layout,
  isProjectTerminalRestoreInProgress: mocks.restoring
}))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn(() => Promise.resolve()) }))

import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { getAllLeafPanes, type LeafNode, useWorkspaceStore } from '@/stores/workspace-store'
import type { Terminal } from '@/types/project'
import { adoptLiveTerminalsOnBoot, recoverTerminalWorkspace } from './terminal-recovery'

function live(id: string, projectId: string, extra: Partial<TerminalStatus> = {}): TerminalStatus {
  return {
    id,
    shell: 'zsh',
    cwd: `/work/${projectId}/${id}`,
    pid: 1,
    cols: 80,
    rows: 24,
    projectId,
    workspaceRefTracked: false,
    latestSeq: 0,
    active: true,
    lifecycle: 'running',
    ...extra
  }
}

function record(id: string, projectId: string, ptyId: string): Terminal {
  return {
    id,
    name: id,
    projectId,
    shell: 'zsh',
    cwd: '/',
    output: [],
    ptyId,
    viewState: 'visible'
  }
}

function tabTerminalIds(): string[] {
  return getAllLeafPanes(useWorkspaceStore.getState().root).flatMap((leaf) =>
    leaf.tabs.flatMap((tab) => (tab.type === 'terminal' ? [tab.terminalId] : []))
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.layout.mockResolvedValue(null)
  mocks.restoring.mockReturnValue(false)
  useProjectStore.setState({
    activeProjectId: 'p1',
    projects: ['p1', 'p2', 'p3'].map((id) => ({ id, name: id }))
  } as never)
  useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
  useWorkspaceStore.setState(() => {
    const root: LeafNode = { type: 'leaf', id: 'pane-root', tabs: [], activeTabId: null }
    return { root, activePaneId: 'pane-root', fullscreenPaneId: null }
  })
})

describe('recoverTerminalWorkspace', () => {
  it('takes the running terminals back into the active project and its tabs', async () => {
    const kept = record('kept', 'p1', 'pty-kept')
    useTerminalStore.getState().setTerminals([kept])
    mocks.list.mockResolvedValue({
      success: true,
      data: [live('pty-kept', 'p1'), live('pty-lost', 'p1')]
    })
    mocks.layout.mockResolvedValue({
      terminals: [{ id: 'old', name: 'api server', shell: 'zsh', cwd: '/', ptyId: 'pty-lost' }],
      activeTerminalId: 'old',
      updatedAt: ''
    })

    const result = await recoverTerminalWorkspace()

    expect(result).toEqual({ adopted: 1, tabsRestored: 2, disconnected: 0 })
    expect(tabTerminalIds().sort()).toEqual(['kept', 'pty-lost'])
    expect(useTerminalStore.getState().findTerminalByPtyId('pty-lost')?.name).toBe('api server')
  })

  it('takes nothing that is not a running terminal of the project', async () => {
    mocks.list.mockResolvedValue({
      success: true,
      data: [
        live('pty-conversation', 'p1', { conversationId: 'c1', workspaceRefTracked: true }),
        live('pty-ended', 'p1', { active: false }),
        live('pty-ssh', '', { projectId: null })
      ]
    })

    const result = await recoverTerminalWorkspace()

    expect(result).toEqual({ adopted: 0, tabsRestored: 0, disconnected: 0 })
    expect(useTerminalStore.getState().terminals).toEqual([])
  })

  it('takes back the running terminals of every project, showing only the active one', async () => {
    useTerminalStore.getState().setTerminals([record('p2-one', 'p2', 'pty-p2-a')])
    mocks.list.mockResolvedValue({
      success: true,
      data: [live('pty-p2-a', 'p2'), live('pty-p2-b', 'p2'), live('pty-p3', 'p3')]
    })

    const result = await recoverTerminalWorkspace()

    expect(result.adopted).toBe(2)
    expect(useTerminalStore.getState().findTerminalByPtyId('pty-p2-b')?.projectId).toBe('p2')
    expect(useTerminalStore.getState().findTerminalByPtyId('pty-p3')?.projectId).toBe('p3')
    // Not the active project: its panes are not on screen.
    expect(tabTerminalIds()).toEqual([])
  })

  it('takes nothing for a project this renderer does not know', async () => {
    mocks.list.mockResolvedValue({ success: true, data: [live('pty-gone-project', 'p9')] })

    const result = await recoverTerminalWorkspace()

    expect(result.adopted).toBe(0)
    expect(useTerminalStore.getState().terminals).toEqual([])
  })

  it('marks a record whose process is gone as disconnected', async () => {
    useTerminalStore.getState().setTerminals([record('gone', 'p1', 'pty-gone')])
    mocks.list.mockResolvedValue({ success: true, data: [] })

    const result = await recoverTerminalWorkspace()

    expect(result.disconnected).toBe(1)
    expect(useTerminalStore.getState().terminals[0].healthStatus).toBe('disconnected')
    expect(tabTerminalIds()).toEqual([])
  })

  it('fails when Terminal Core cannot be asked', async () => {
    mocks.list.mockResolvedValue({ success: false, error: 'core down', code: 'X' })
    await expect(recoverTerminalWorkspace()).rejects.toThrow('core down')
  })
})

describe('adoptLiveTerminalsOnBoot', () => {
  it('takes back other projects and leaves the active and restoring ones to their restore', async () => {
    mocks.restoring.mockImplementation((projectId: string) => projectId === 'p2')
    mocks.list.mockResolvedValue({
      success: true,
      data: [live('pty-p1', 'p1'), live('pty-p2', 'p2'), live('pty-p3', 'p3')]
    })
    mocks.layout.mockResolvedValue({
      terminals: [{ id: 'old', name: 'worker', shell: 'zsh', cwd: '/', ptyId: 'pty-p3' }],
      activeTerminalId: 'old',
      updatedAt: ''
    })

    expect(await adoptLiveTerminalsOnBoot()).toBe(1)

    const store = useTerminalStore.getState()
    expect(store.findTerminalByPtyId('pty-p1')).toBeUndefined()
    expect(store.findTerminalByPtyId('pty-p2')).toBeUndefined()
    expect(store.findTerminalByPtyId('pty-p3')?.name).toBe('worker')
    expect(tabTerminalIds()).toEqual([])
  })

  it('backs off when the project starts its own restore while its layout loads', async () => {
    let restoring = false
    mocks.restoring.mockImplementation(() => restoring)
    mocks.layout.mockImplementation(async () => {
      restoring = true
      return null
    })
    mocks.list.mockResolvedValue({ success: true, data: [live('pty-p3', 'p3')] })

    expect(await adoptLiveTerminalsOnBoot()).toBe(0)
    expect(useTerminalStore.getState().findTerminalByPtyId('pty-p3')).toBeUndefined()
  })
})
