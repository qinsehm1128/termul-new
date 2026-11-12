import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import type { LeafNode } from '@/types/workspace.types'
import { TerminalSwitcherBar } from './TerminalSwitcherBar'

// Deliberately NOT mocking `@/lib/terminal-board-navigation`: these tests cover
// the click's visible outcome (which pane tab is showing), not just that the
// handler was called. The bar's other test file mocks that module, which is why
// the dead click went unnoticed there.
const navigate = vi.fn()
vi.mock('react-router-dom', async (importOriginal) => {
  const actual = await importOriginal<typeof import('react-router-dom')>()
  return {
    useNavigate: () => navigate,
    useMatch: (pattern: string) => actual.matchPath(pattern, '/')
  }
})

vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

/**
 * One terminal per project, and an editor tab in front of it — the state a
 * user is in when they click a project chip to get back to its terminal.
 */
function seed(activeProjectId: string): void {
  useProjectStore.setState({
    projects: [
      { id: 'p1', name: 'P1', color: 'blue' },
      { id: 'p2', name: 'P2', color: 'green' }
    ],
    groups: [],
    activeProjectId,
    activeGroupId: null
  })
  useTerminalStore.setState({
    terminals: [
      {
        id: 't1',
        name: 'Termul',
        projectId: 'p1',
        shell: 'zsh',
        ptyId: 'pty-1',
        viewState: 'visible'
      },
      {
        id: 't2',
        name: 'Qscode',
        projectId: 'p2',
        shell: 'zsh',
        ptyId: 'pty-2',
        viewState: 'visible'
      }
    ],
    activeTerminalId: 't1',
    recentTerminalIds: ['t1']
  })
  const root: LeafNode = {
    type: 'leaf',
    id: 'pane-root',
    tabs: [{ type: 'editor', id: 'edit-/a.ts', filePath: '/a.ts' }],
    activeTabId: 'edit-/a.ts'
  }
  useWorkspaceStore.setState({ root, activePaneId: 'pane-root', agentLauncherPaneId: null })
}

function pane(): LeafNode {
  return useWorkspaceStore.getState().root as LeafNode
}

describe('TerminalSwitcherBar click outcome', () => {
  beforeEach(() => {
    navigate.mockClear()
    seed('p1')
  })

  it("brings the active project's terminal forward when its chip is clicked", () => {
    // The reported symptom: clicking the chip does nothing. The chip's project
    // is already active, so no project change / route change fires the deferred
    // focus — the click must land the terminal itself.
    render(<TerminalSwitcherBar />)

    fireEvent.click(screen.getByRole('tab', { name: /P1/ }))

    expect(pane().activeTabId).toBe('term-t1')
    expect(pane().tabs.map((tab) => tab.id)).toContain('term-t1')
  })

  it('switches project and terminal when another project chip is clicked', () => {
    render(<TerminalSwitcherBar />)

    fireEvent.click(screen.getByRole('tab', { name: /P2/ }))

    expect(useProjectStore.getState().activeProjectId).toBe('p2')
    expect(useTerminalStore.getState().activeTerminalId).toBe('t2')
  })
})
