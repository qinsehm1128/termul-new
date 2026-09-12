import { render, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Terminal } from '@/types/project'
import type { LeafNode } from '@/types/workspace.types'

/**
 * `ConnectedTerminal` is `memo()`'d, but PaneContent used to hand it a fresh
 * `spawnOptions` literal and three inline arrows on every render — so the memo
 * never held. PaneContent re-renders whenever ANY terminal in the pane reports
 * activity (a busy terminal does so every couple of seconds) and on every tab
 * switch, which meant every terminal in the pane re-rendered each time.
 *
 * These tests pin the two halves of the contract: identity survives churn that
 * changes nothing the props capture, and identity changes when a captured
 * value does (the component copies these callbacks into refs during render, so
 * a skipped render would otherwise leave a stale closure behind).
 */

const { capturedProps, terminalState, mockSetTerminalPtyId, mockAddCommandToHistory } = vi.hoisted(
  () => ({
    capturedProps: [] as Record<string, unknown>[],
    terminalState: { terminals: [] as Terminal[] },
    mockSetTerminalPtyId: vi.fn(),
    mockAddCommandToHistory: vi.fn()
  })
)

vi.mock('@/components/terminal/ConnectedTerminal', () => ({
  ConnectedTerminal: (props: Record<string, unknown>) => {
    capturedProps.push(props)
    return <div data-testid="connected-terminal-stub" />
  }
}))

vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: vi.fn((selector: (s: typeof terminalState) => unknown) =>
    selector(terminalState)
  ),
  useTerminalActions: vi.fn(() => ({ setTerminalPtyId: mockSetTerminalPtyId }))
}))

vi.mock('@/hooks/use-command-history', () => ({
  useAddCommand: () => mockAddCommandToHistory
}))

vi.mock('@/stores/project-store', () => ({
  useProjectStore: vi.fn((selector: (s: { activeProjectId: string }) => unknown) =>
    selector({ activeProjectId: 'proj-1' })
  )
}))

vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) =>
    selector({
      root: { type: 'leaf', id: 'pane-1', tabs: [], activeTabId: null },
      activePaneId: 'pane-1',
      fullscreenPaneId: null,
      agentLauncherPaneId: null,
      setActivePane: vi.fn()
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
vi.mock('@/components/agents/AgentLauncher', () => ({ AgentLauncher: () => null }))
vi.mock('@/components/agents/AgentIcon', () => ({ AgentIcon: () => null }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

import { PaneContent } from './PaneContent'

const terminalPane: LeafNode = {
  type: 'leaf',
  id: 'pane-1',
  activeTabId: 'tab-term-1',
  tabs: [{ type: 'terminal', id: 'tab-term-1', terminalId: 'term-1' }]
}

function makeTerminal(overrides: Partial<Terminal> = {}): Terminal {
  return {
    id: 'term-1',
    name: 'Terminal 1',
    projectId: 'proj-1',
    shell: 'bash',
    cwd: '/work',
    ptyId: 'pty-1',
    ...overrides
  } as Terminal
}

function renderPane() {
  return render(
    <MemoryRouter>
      <PaneContent pane={terminalPane} />
    </MemoryRouter>
  )
}

describe('PaneContent — ConnectedTerminal prop stability (AC7)', () => {
  beforeEach(() => {
    capturedProps.length = 0
    terminalState.terminals = [makeTerminal()]
    vi.clearAllMocks()
  })

  it('keeps spawnOptions and the callbacks referentially stable across activity churn', async () => {
    const { rerender } = renderPane()
    await waitFor(() => expect(capturedProps.length).toBeGreaterThan(0))
    const first = capturedProps[capturedProps.length - 1]

    // What an activity update looks like: a brand-new terminal object inside a
    // brand-new array, with nothing the props capture actually changed.
    terminalState.terminals = [makeTerminal({ hasActivity: true })]
    rerender(
      <MemoryRouter>
        <PaneContent pane={terminalPane} />
      </MemoryRouter>
    )
    await waitFor(() => expect(capturedProps.length).toBeGreaterThan(1))
    const second = capturedProps[capturedProps.length - 1]

    expect(second.spawnOptions).toBe(first.spawnOptions)
    expect(second.onCommand).toBe(first.onCommand)
    expect(second.onBoundToStoreTerminal).toBe(first.onBoundToStoreTerminal)
  })

  it('gives the callbacks a new identity when a value they capture changes', async () => {
    const { rerender } = renderPane()
    await waitFor(() => expect(capturedProps.length).toBeGreaterThan(0))
    const first = capturedProps[capturedProps.length - 1]

    // The component copies these callbacks into refs during render, so a
    // renamed terminal MUST produce new identities — otherwise memo would skip
    // the render and command history would keep filing under the old name.
    terminalState.terminals = [makeTerminal({ name: 'Renamed' })]
    rerender(
      <MemoryRouter>
        <PaneContent pane={terminalPane} />
      </MemoryRouter>
    )
    await waitFor(() => expect(capturedProps.length).toBeGreaterThan(1))
    const second = capturedProps[capturedProps.length - 1]

    expect(second.onCommand).not.toBe(first.onCommand)
  })

  it('files command history against the terminal values it captured', async () => {
    renderPane()
    await waitFor(() => expect(capturedProps.length).toBeGreaterThan(0))
    const { onCommand } = capturedProps[capturedProps.length - 1] as {
      onCommand: (command: string) => void
    }

    onCommand('ls -la')

    expect(mockAddCommandToHistory).toHaveBeenCalledWith('ls -la', 'Terminal 1', 'term-1', 'proj-1')
  })

  it('binds a newly spawned pty through the captured terminal id', async () => {
    renderPane()
    await waitFor(() => expect(capturedProps.length).toBeGreaterThan(0))
    const { onBoundToStoreTerminal } = capturedProps[capturedProps.length - 1] as {
      onBoundToStoreTerminal: (ptyId: string) => void
    }

    onBoundToStoreTerminal('pty-new')
    expect(mockSetTerminalPtyId).toHaveBeenCalledWith('term-1', 'pty-new')

    // Already bound — must not churn the store.
    mockSetTerminalPtyId.mockClear()
    onBoundToStoreTerminal('pty-1')
    expect(mockSetTerminalPtyId).not.toHaveBeenCalled()
  })
})
