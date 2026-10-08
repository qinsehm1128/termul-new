import { fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Terminal } from '@/types/project'
import type { LeafNode } from '@/types/workspace.types'

/**
 * Pressing inside a terminal is pressing inside its pane, so it must make that
 * pane the active one — right-click included, since that is how the split menu
 * is reached.
 *
 * The terminal surface stops `mousedown` from bubbling (ConnectedTerminal's
 * wrapper keeps parent handlers from stealing xterm's focus back). Pane
 * activation listening in the bubble phase therefore never heard a click in a
 * terminal: the user worked in pane A while the store still named the last
 * active pane C, and everything that defaults to the active pane went to C.
 * The stub reproduces exactly that surface behaviour.
 */

const { mockSetActivePane, terminalState } = vi.hoisted(() => ({
  mockSetActivePane: vi.fn(),
  terminalState: { terminals: [] as Terminal[] }
}))

vi.mock('@/components/terminal/ConnectedTerminal', () => ({
  ConnectedTerminal: () => (
    // biome-ignore lint/a11y/noStaticElementInteractions: test stub of the xterm surface
    <div data-testid="terminal-surface" onMouseDown={(event) => event.stopPropagation()} />
  )
}))

vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: Object.assign(
    vi.fn((selector: (s: typeof terminalState) => unknown) => selector(terminalState)),
    { getState: () => terminalState }
  ),
  useTerminalActions: vi.fn(() => ({ setTerminalPtyId: vi.fn() }))
}))

vi.mock('@/hooks/use-command-history', () => ({ useAddCommand: () => vi.fn() }))

vi.mock('@/stores/project-store', () => ({
  useProjectStore: vi.fn((selector: (s: { activeProjectId: string }) => unknown) =>
    selector({ activeProjectId: 'proj-1' })
  )
}))

vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) =>
    selector({
      root: { type: 'leaf', id: 'pane-C', tabs: [], activeTabId: null },
      activePaneId: 'pane-C',
      fullscreenPaneId: null,
      agentLauncherPaneId: null,
      setActivePane: mockSetActivePane
    })
  ),
  getAllLeafPanes: () => [],
  retireTerminalRecord: vi.fn()
}))

vi.mock('@/hooks/use-mobile-web-shell', () => ({ useMobileWebShell: () => false }))
vi.mock('@/hooks/use-pane-dnd', () => ({
  usePaneDnd: () => ({ isDragging: false, previewTarget: null })
}))
vi.mock('@/components/workspace/WorkspaceTabBar', () => ({
  WorkspaceTabBar: () => <div data-testid="tabbar-stub" />
}))
vi.mock('@/components/workspace/DropZoneOverlay', () => ({ DropZoneOverlay: () => null }))
vi.mock('@/features/agent-session/agents/AgentLauncher', () => ({ AgentLauncher: () => null }))
vi.mock('@/features/agent-session/agents/AgentIcon', () => ({ AgentIcon: () => null }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

import { PaneContent } from './PaneContent'

const paneA: LeafNode = {
  type: 'leaf',
  id: 'pane-A',
  activeTabId: 'tab-term-a',
  tabs: [{ type: 'terminal', id: 'tab-term-a', terminalId: 'term-a' }]
}

describe('PaneContent — pressing inside a terminal activates its pane', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    terminalState.terminals = [
      {
        id: 'term-a',
        name: 'Terminal A',
        projectId: 'proj-1',
        shell: 'bash',
        cwd: '/work',
        ptyId: 'pty-a'
      } as Terminal
    ]
  })

  it.each([
    ['left', 0],
    ['right', 2]
  ])('activates the pane on a %s-button press in its terminal', async (_label, button) => {
    render(
      <MemoryRouter>
        <PaneContent pane={paneA} />
      </MemoryRouter>
    )

    fireEvent.mouseDown(await screen.findByTestId('terminal-surface'), { button })

    expect(mockSetActivePane).toHaveBeenCalledWith('pane-A')
  })
})
