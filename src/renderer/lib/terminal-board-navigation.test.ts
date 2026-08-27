import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { useSSHStore } from '@/stores/ssh-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import type { LeafNode } from '@/types/workspace.types'
import {
  applyPendingTerminalFocus,
  clearPendingTerminalFocus,
  openBoardProject,
  openBoardTerminal,
  peekPendingTerminalFocus
} from './terminal-board-navigation'

const { logFrontendError } = vi.hoisted(() => ({
  logFrontendError: vi.fn()
}))

vi.mock('@/lib/log-api', () => ({ logFrontendError }))

describe('terminal-board-navigation', () => {
  beforeEach(() => {
    clearPendingTerminalFocus()
    logFrontendError.mockClear()
    useProjectStore.setState({
      projects: [{ id: 'p-cost', name: 'cost', color: 'blue', path: '/srv/logistics' }],
      groups: [],
      activeProjectId: '',
      activeGroupId: null,
      isLoaded: true
    })
    useSSHStore.setState({ selectedProfileId: 'ssh-1' } as never)
    useTerminalStore.setState({
      terminals: [
        {
          id: 'term-1',
          name: 'cost',
          projectId: 'p-cost',
          shell: 'zsh',
          ptyId: 'pty-1',
          healthStatus: 'running',
          viewState: 'visible'
        }
      ],
      activeTerminalId: '',
      ptyIdIndex: new Map([['pty-1', 'term-1']]),
      cleanupRecoveries: {}
    })
    const root: LeafNode = { type: 'leaf', id: 'pane-root', tabs: [], activeTabId: null }
    useWorkspaceStore.setState({
      root,
      activePaneId: 'pane-root',
      agentLauncherPaneId: 'pane-root'
    })
  })

  it('selects the project and defers tab focus until the workspace is ready', () => {
    const navigate = vi.fn()
    openBoardTerminal({ projectId: 'p-cost', terminalId: 'term-1', navigate })

    expect(navigate).toHaveBeenCalledWith('/')
    expect(useProjectStore.getState().activeProjectId).toBe('p-cost')
    expect(peekPendingTerminalFocus()).toEqual({ projectId: 'p-cost', terminalId: 'term-1' })
    expect(useWorkspaceStore.getState().agentLauncherPaneId).toBeNull()
    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ source: 'terminal-board.open' })
    )

    expect(applyPendingTerminalFocus('p-cost')).toBe(true)
    expect(peekPendingTerminalFocus()).toEqual({ projectId: 'p-cost', terminalId: 'term-1' })
    const pane = useWorkspaceStore.getState().root as LeafNode
    expect(pane.activeTabId).toBe('term-term-1')
    expect(pane.tabs).toEqual([{ type: 'terminal', id: 'term-term-1', terminalId: 'term-1' }])

    useWorkspaceStore.setState({
      root: { type: 'leaf', id: 'pane-root', tabs: [], activeTabId: null }
    })
    expect(applyPendingTerminalFocus('p-cost')).toBe(true)
    expect((useWorkspaceStore.getState().root as LeafNode).activeTabId).toBe('term-term-1')
  })

  it('opens a project without focusing a terminal', () => {
    const navigate = vi.fn()
    openBoardTerminal({ projectId: 'p-cost', terminalId: 'term-1', navigate })
    openBoardProject({ projectId: 'p-cost', navigate })
    expect(navigate).toHaveBeenLastCalledWith('/')
    expect(useProjectStore.getState().activeProjectId).toBe('p-cost')
    expect(peekPendingTerminalFocus()).toBeNull()
  })

  it('focuses an unassigned terminal without a deferred project switch', () => {
    const navigate = vi.fn()
    openBoardTerminal({ projectId: null, terminalId: 'term-1', navigate })
    expect(navigate).toHaveBeenCalledWith('/')
    expect(peekPendingTerminalFocus()).toBeNull()
    expect((useWorkspaceStore.getState().root as LeafNode).activeTabId).toBe('term-term-1')
  })
})
