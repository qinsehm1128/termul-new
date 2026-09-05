import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ConversationTerminalButton } from './ConversationTerminalButton'

const {
  mockSpawnTerminalInPane,
  mockTerminateTerminalResource,
  mockGetDefaultCwdForProject,
  state
} = vi.hoisted(() => ({
  mockSpawnTerminalInPane: vi.fn(),
  mockTerminateTerminalResource: vi.fn(),
  mockGetDefaultCwdForProject: vi.fn(),
  state: {
    conversationId: 'conv-1' as string | null,
    workspaceCwd: '/work/conv-1' as string | undefined,
    activePaneId: 'pane-1' as string | null,
    activeProjectId: 'proj-1',
    terminals: [] as Array<{ id: string; name: string; conversationId?: string }>
  }
}))

vi.mock('@/stores/conversation-store', () => ({
  useConversationStore: (selector: (s: unknown) => unknown) =>
    selector({
      activeConversationId: state.conversationId,
      summariesById: state.conversationId
        ? { [state.conversationId]: { workspaceCwd: state.workspaceCwd } }
        : {}
    })
}))

vi.mock('@/stores/terminal-store', () => ({
  useConversationTerminals: (conversationId: string | null) =>
    conversationId ? state.terminals.filter((t) => t.conversationId === conversationId) : [],
  useTerminalStore: {
    getState: () => ({ terminateTerminalResource: mockTerminateTerminalResource })
  }
}))

vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: { getState: () => ({ activePaneId: state.activePaneId }) }
}))

vi.mock('@/stores/project-store', () => ({
  useProjectStore: { getState: () => ({ activeProjectId: state.activeProjectId }) }
}))

vi.mock('@/stores/app-settings-store', () => ({
  useAppSettingsStore: { getState: () => ({ settings: { maxTerminalsPerProject: 10 } }) }
}))

vi.mock('@/lib/terminal-spawn', () => ({
  spawnTerminalInPane: mockSpawnTerminalInPane
}))

vi.mock('@/lib/worktree-context', () => ({
  getDefaultCwdForProject: mockGetDefaultCwdForProject
}))

vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

describe('ConversationTerminalButton', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    state.conversationId = 'conv-1'
    state.workspaceCwd = '/work/conv-1'
    state.activePaneId = 'pane-1'
    state.activeProjectId = 'proj-1'
    state.terminals = []
    mockSpawnTerminalInPane.mockResolvedValue({ success: true, terminalId: 'term-1' })
    mockTerminateTerminalResource.mockResolvedValue(true)
    mockGetDefaultCwdForProject.mockReturnValue('/work/proj-1')
  })

  afterEach(cleanup)

  it('renders nothing outside a conversation', () => {
    state.conversationId = null
    render(<ConversationTerminalButton />)
    expect(screen.queryByTestId('conversation-terminal-start')).toBeNull()
    expect(screen.queryByTestId('conversation-terminal-badge')).toBeNull()
  })

  /**
   * The first terminal has to be spawnable with none already open, so the cwd
   * cannot be read off a sibling — it comes from the conversation's own
   * workspace directory, the same rule the pane's own create path uses.
   */
  it('spawns the first terminal scoped to the conversation, in its workspace directory', async () => {
    render(<ConversationTerminalButton />)

    fireEvent.click(screen.getByTestId('conversation-terminal-start'))

    await waitFor(() => {
      expect(mockSpawnTerminalInPane).toHaveBeenCalledWith(
        'pane-1',
        'proj-1',
        '/work/conv-1',
        expect.objectContaining({ conversationId: 'conv-1' })
      )
    })
    expect(mockGetDefaultCwdForProject).not.toHaveBeenCalled()
  })

  it('falls back to the project directory when the conversation has none', async () => {
    state.workspaceCwd = undefined
    render(<ConversationTerminalButton />)

    fireEvent.click(screen.getByTestId('conversation-terminal-start'))

    await waitFor(() => {
      expect(mockSpawnTerminalInPane).toHaveBeenCalledWith(
        'pane-1',
        'proj-1',
        '/work/proj-1',
        expect.anything()
      )
    })
  })

  /**
   * The whole point of the control. A conversation terminal's *tab* × only
   * retires the view and leaves the PTY running — that is the "stays resident
   * in the background" the user hit. This × has to actually end the process.
   */
  it('terminates the process rather than only closing its view', async () => {
    state.terminals = [{ id: 'term-1', name: 'Terminal 1', conversationId: 'conv-1' }]
    render(<ConversationTerminalButton />)

    fireEvent.click(screen.getByTestId('conversation-terminal-badge'))
    const row = await screen.findByTestId('conversation-terminal-row')
    fireEvent.click(row.querySelector('button') as HTMLElement)

    await waitFor(() => {
      expect(mockTerminateTerminalResource).toHaveBeenCalledWith('term-1')
    })
  })

  it('counts only this conversation’s terminals', () => {
    state.terminals = [
      { id: 'term-1', name: 'Terminal 1', conversationId: 'conv-1' },
      { id: 'term-2', name: 'Terminal 2', conversationId: 'conv-other' },
      { id: 'term-3', name: 'Terminal 3', conversationId: 'conv-1' }
    ]
    render(<ConversationTerminalButton />)

    expect(screen.getByTestId('conversation-terminal-badge')).toHaveTextContent('2')
  })

  it('does not spawn when there is no pane to spawn into', async () => {
    state.activePaneId = null
    render(<ConversationTerminalButton />)

    fireEvent.click(screen.getByTestId('conversation-terminal-start'))

    await waitFor(() => {
      expect(mockSpawnTerminalInPane).not.toHaveBeenCalled()
    })
  })
})
