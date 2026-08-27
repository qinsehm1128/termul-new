import { fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useConversationStore } from '@/stores/conversation-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { MobileChatShell } from './MobileChatShell'

const {
  mockNavigate,
  mockCloseChatView,
  mockDetachBinding,
  mockRebindBinding,
  mockSuspendBinding,
  mockReplaceBinding,
  mockDeleteConversation,
  mockSetActiveTab,
  mockLogFrontendError,
  projectRef,
  tauriRef,
  workspaceRef,
  mockReopenTerminalView
} = vi.hoisted(() => ({
  mockNavigate: vi.fn(),
  mockCloseChatView: vi.fn(),
  mockDetachBinding: vi.fn(),
  mockRebindBinding: vi.fn(),
  mockSuspendBinding: vi.fn(),
  mockReplaceBinding: vi.fn(),
  mockDeleteConversation: vi.fn(),
  mockSetActiveTab: vi.fn(),
  mockLogFrontendError: vi.fn(),
  // Mutable so individual tests can flip the active project path (the Git
  // Changes overflow item is disabled when `activeProject.path` is missing)
  // and the shell into web/remote mode (where the project-switcher +
  // files tab are mounted).
  projectRef: {
    current: { id: 'p1', name: 'Demo', path: '/demo' } as {
      id?: string
      name?: string
      path?: string
    }
  },
  tauriRef: { current: true as boolean },
  workspaceRef: {
    panes: [
      {
        type: 'leaf' as const,
        id: 'pane-1',
        tabs: [{ type: 'agent-chat' as const, id: 'tab-1', sessionId: 's1' }],
        activeTabId: 'tab-1'
      }
    ]
  },
  mockReopenTerminalView: vi.fn()
}))

function defaultPanes(): typeof workspaceRef.panes {
  return [
    {
      type: 'leaf',
      id: 'pane-1',
      tabs: [{ type: 'agent-chat', id: 'tab-1', sessionId: 's1' }],
      activeTabId: 'tab-1'
    }
  ]
}

function openOverflow(): void {
  fireEvent.click(screen.getByLabelText('More'))
}

vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom')
  return {
    ...actual,
    useNavigate: () => mockNavigate
  }
})

vi.mock('@/stores/project-store', () => ({
  useActiveProject: () => projectRef.current,
  useProjectStore: (selector: (state: { projects: unknown[] }) => unknown) =>
    selector({ projects: projectRef.current.id ? [projectRef.current] : [] })
}))

vi.mock('@/stores/workspace-store', () => ({
  getAllLeafPanes: () => workspaceRef.panes,
  useWorkspaceStore: Object.assign(
    (sel: (s: { root: unknown; activePaneId: string }) => unknown) =>
      sel({ root: {}, activePaneId: 'pane-1' }),
    {
      getState: () => ({
        root: {},
        activePaneId: 'pane-1',
        setActiveTab: mockSetActiveTab,
        reopenTerminalView: mockReopenTerminalView
      })
    }
  )
}))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: (...args: unknown[]) => mockLogFrontendError(...args)
}))

vi.mock('@/stores/acp-store', () => ({
  useAcpStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      sessions: {
        s1: {
          title: 'Hello chat',
          conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
        }
      },
      sessionIndex: [],
      closeChatView: mockCloseChatView,
      detachAgentBinding: mockDetachBinding,
      rebindDetachedBinding: mockRebindBinding,
      suspendAgentBinding: mockSuspendBinding,
      replaceAgentBinding: mockReplaceBinding,
      deleteConversation: mockDeleteConversation
    })
}))

vi.mock('@/components/chat/ChatHistoryTab', () => ({
  ChatHistoryTab: ({ onSessionOpened }: { onSessionOpened?: () => void }) => (
    <button type="button" onClick={() => onSessionOpened?.()}>
      Open history chat
    </button>
  )
}))

// Stub the drawer so the shell test focuses on the trigger wiring (button →
// projectsOpen → drawer `open` prop → onOpenChange close). The drawer's own
// open/close + state rendering is covered in ProjectSwitcherDrawer.test.tsx.
vi.mock('@/components/chat/ProjectSwitcherDrawer', () => ({
  ProjectSwitcherDrawer: ({
    open,
    onOpenChange
  }: {
    open: boolean
    onOpenChange: (open: boolean) => void
  }) =>
    open ? (
      <div>
        <span>project-drawer</span>
        <button type="button" onClick={() => onOpenChange(false)}>
          close-drawer
        </button>
      </div>
    ) : null
}))

// Stub the file-explorer drawer so the shell test focuses on the trigger
// wiring (button → filesOpen → drawer `open` prop → onOpenChange close).
// The drawer's own open/close + file-management is covered in
// MobileFileExplorer.test.tsx.
vi.mock('@/components/cli-sessions/CliSessionPanel', () => ({
  CliSessionPanel: () => <div>cli-session-panel</div>
}))

vi.mock('./MobileFileExplorer', () => ({
  MobileFileExplorer: ({
    open,
    variant,
    onOpenChange
  }: {
    open?: boolean
    variant?: 'page' | 'sheet'
    onOpenChange?: (open: boolean) => void
  }) =>
    variant === 'page' ? (
      <div data-mobile-file-explorer="page">files-page</div>
    ) : open ? (
      <div>
        <span>files-drawer</span>
        <button type="button" onClick={() => onOpenChange?.(false)}>
          close-files
        </button>
      </div>
    ) : null
}))

vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => tauriRef.current
}))

describe('MobileChatShell', () => {
  beforeEach(() => {
    mockNavigate.mockReset()
    mockCloseChatView.mockReset()
    mockDetachBinding.mockReset()
    mockRebindBinding.mockReset()
    mockSuspendBinding.mockReset()
    mockReplaceBinding.mockReset()
    mockDeleteConversation.mockReset()
    mockReopenTerminalView.mockReset()
    mockSetActiveTab.mockReset()
    mockLogFrontendError.mockReset()
    workspaceRef.panes = defaultPanes()
    useConversationStore.getState().reset()
    useConversationStore.getState().replaceSummaries([
      {
        schemaVersion: 2,
        conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
        createdAtUtc: '2026-08-15T09:45:15.123Z',
        creationPartition: { year: 2026, month: 8, day: 15, path: '2026/08/15' },
        workspaceCwd: '/visible/sessions/2026/08/15/conversation',
        executionTarget: { kind: 'workspace' },
        projectAttachment: null,
        lifecycleState: 'ready',
        lastSeq: 4,
        createdBy: 'termul'
      }
    ])
    useConversationStore.getState().setActiveConversationId('018f7a1c-1b4d-7c8a-9f01-0123456789ab')
    useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
    tauriRef.current = true
    projectRef.current = { id: 'p1', name: 'Demo', path: '/demo' }
  })

  it('renders slim header with title and no desktop chrome markers', () => {
    const { container } = render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    expect(screen.getByText('Hello chat')).toBeInTheDocument()
    expect(screen.getByText('chat body')).toBeInTheDocument()
    expect(screen.getByLabelText('Open menu')).toBeInTheDocument()
    expect(screen.getByLabelText('More')).toBeInTheDocument()
    expect(screen.queryByLabelText('New chat')).not.toBeInTheDocument()
    expect(document.querySelector('[data-mobile-chat-shell]')).toBeTruthy()
    expect(document.querySelector('[data-od-id="workspace-tabs"]')).toBeTruthy()
    expect(screen.getByRole('tab', { name: 'Chat' })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByRole('tab', { name: 'Terminal' })).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Files' })).toBeInTheDocument()
    // Header title is a heading for screen-reader landmark navigation.
    expect(container.querySelector('h1')?.textContent).toBe('Hello chat')
    // Desktop chrome (persistent sidebar, activity rail) must not render inside
    // the mobile shell — assert their markers are absent.
    expect(container.querySelector('[data-sidebar]')).toBeNull()
    // The menu button reflects drawer state for assistive tech.
    expect(screen.getByLabelText('Open menu')).toHaveAttribute('aria-expanded', 'false')
  })

  it('opens the mobile execution-target sheet with immutable Conversation identity', async () => {
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    const trigger = screen.getByRole('button', {
      name: 'Execution target and project context'
    })
    expect(trigger).toHaveClass('h-11')
    fireEvent.click(trigger)
    expect(
      await screen.findByText(
        "Attach or detach project context and change this Conversation's execution target without changing its identity or workspace."
      )
    ).toBeInTheDocument()
    expect(screen.getByLabelText('Execution target')).toHaveTextContent('Conversation workspace')
    expect(screen.getByText('018f7a1c-1b4d-7c8a-9f01-0123456789ab')).toBeInTheDocument()
    expect(screen.getByText('2026-08-15T09:45:15.123Z')).toBeInTheDocument()
    expect(screen.getAllByText('/visible/sessions/2026/08/15/conversation').length).toBeGreaterThan(
      0
    )
  })

  it('exposes touch-sized independent Conversation lifecycle actions for the active chat', () => {
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    const actions = screen.getByRole('button', {
      name: 'Conversation actions for Hello chat'
    })
    expect(actions).toHaveClass('size-11')
    fireEvent.pointerDown(actions, { button: 0, ctrlKey: false })
    expect(screen.getByText('Close chat view')).toBeInTheDocument()
    expect(screen.getByText('Detach binding')).toBeInTheDocument()
    expect(screen.getByText('Rebind detached agent')).toBeInTheDocument()
    expect(screen.getByText('Suspend agent')).toBeInTheDocument()
    expect(screen.getByText('Replace agent')).toBeInTheDocument()
    expect(screen.getByText('Delete conversation')).toBeInTheDocument()
  })

  it('opens the chat drawer and closes it after selecting a session', async () => {
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    fireEvent.click(screen.getByLabelText('Open menu'))
    expect(screen.getByLabelText('Open menu')).toHaveAttribute('aria-expanded', 'true')
    expect(await screen.findByText('Open history chat')).toBeInTheDocument()
    expect(screen.getByText('New chat')).toBeInTheDocument()

    fireEvent.click(screen.getByText('Open history chat'))
    expect(screen.queryByText('Open history chat')).not.toBeInTheDocument()
    expect(screen.getByLabelText('Open menu')).toHaveAttribute('aria-expanded', 'false')
  })

  it('enables project-less New chat by default', () => {
    projectRef.current = {}
    const onNewChat = vi.fn()
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={onNewChat}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    const newChat = screen.getByLabelText('New chat')
    expect(newChat).toBeEnabled()
    fireEvent.click(newChat)
    expect(onNewChat).toHaveBeenCalledTimes(1)
  })

  it('opens the terminal board from the overflow menu', () => {
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    fireEvent.click(screen.getByLabelText('Terminal board'))
    expect(mockNavigate).toHaveBeenCalledWith('/terminals')
  })

  it('invokes onNewChat from the overflow menu', () => {
    const onNewChat = vi.fn()
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={onNewChat} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    fireEvent.click(screen.getByLabelText('New chat'))
    expect(onNewChat).toHaveBeenCalledTimes(1)
  })

  it('hides the Switch project button in Tauri (desktop) mode', () => {
    tauriRef.current = true
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )
    // Desktop never mounts the web/remote project drawer — the sidebar owns
    // project switching there. The trigger must not leak into header or overflow.
    expect(screen.queryByLabelText('Switch project')).not.toBeInTheDocument()
    openOverflow()
    expect(screen.queryByLabelText('Switch project')).not.toBeInTheDocument()
  })

  it('mounts the project drawer trigger in web mode and toggles it open/closed', async () => {
    tauriRef.current = false
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    const switchBtn = screen.getByLabelText('Switch project')
    expect(switchBtn).toBeInTheDocument()
    // Drawer starts closed.
    expect(screen.queryByText('project-drawer')).not.toBeInTheDocument()

    fireEvent.click(switchBtn)
    expect(await screen.findByText('project-drawer')).toBeInTheDocument()

    // Closing via the drawer's onOpenChange(false) unmounts its content.
    fireEvent.click(screen.getByText('close-drawer'))
    expect(screen.queryByText('project-drawer')).not.toBeInTheDocument()
  })

  it('mounts the CLI sessions sheet in web mode', async () => {
    tauriRef.current = false
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    fireEvent.click(screen.getByLabelText('CLI sessions'))
    expect(await screen.findByText('cli-session-panel')).toBeInTheDocument()
  })

  it('renders the file explorer inline on the Files tab in web mode', async () => {
    tauriRef.current = false
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    expect(screen.queryByLabelText('Browse files')).not.toBeInTheDocument()
    expect(document.querySelector('[data-mobile-workspace-surface="files"]')).toHaveClass('hidden')

    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    expect(await screen.findByText('files-page')).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Files' })).toHaveAttribute('aria-selected', 'true')
    expect(document.querySelector('[data-mobile-workspace-surface="pane"]')).toHaveClass('hidden')
    expect(document.querySelector('[data-mobile-workspace-surface="files"]')).not.toHaveClass(
      'hidden'
    )
    expect(screen.getByText('chat body')).toBeInTheDocument()
  })

  it('hides the Browse files button in Tauri (desktop) mode', () => {
    tauriRef.current = true
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )
    // Desktop never mounts the web/remote file explorer — the right-sidebar
    // FileExplorer owns file browsing there.
    expect(screen.queryByLabelText('Browse files')).not.toBeInTheDocument()
    expect(screen.queryByLabelText('CLI sessions')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    expect(screen.getByText('Use the desktop file explorer')).toBeInTheDocument()
    expect(screen.queryByText('files-page')).not.toBeInTheDocument()
  })

  it('hides the Command palette button in Tauri (desktop) mode', () => {
    tauriRef.current = true
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat onOpenCommandPalette={vi.fn()}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )
    expect(screen.queryByLabelText('Command palette')).not.toBeInTheDocument()
    openOverflow()
    expect(screen.queryByLabelText('Command palette')).not.toBeInTheDocument()
  })

  it('mounts the Command palette trigger in web mode and invokes onOpenCommandPalette', () => {
    tauriRef.current = false
    const onOpenCommandPalette = vi.fn()
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat onOpenCommandPalette={onOpenCommandPalette}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    const btn = screen.getByLabelText('Command palette')
    fireEvent.click(btn)
    expect(onOpenCommandPalette).toHaveBeenCalledTimes(1)
  })

  it('hides the Git changes button in Tauri (desktop) mode', () => {
    tauriRef.current = true
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat onOpenGitChanges={vi.fn()}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )
    expect(screen.queryByLabelText('Git changes')).not.toBeInTheDocument()
    openOverflow()
    expect(screen.queryByLabelText('Git changes')).not.toBeInTheDocument()
  })

  it('mounts the Git changes trigger in web mode and invokes onOpenGitChanges', () => {
    tauriRef.current = false
    const onOpenGitChanges = vi.fn()
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat onOpenGitChanges={onOpenGitChanges}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    const btn = screen.getByLabelText('Git changes')
    expect(btn).not.toBeDisabled()
    fireEvent.click(btn)
    expect(onOpenGitChanges).toHaveBeenCalledTimes(1)
  })

  it('disables the Git changes button when no active project path', () => {
    tauriRef.current = false
    projectRef.current = { id: 'p1', name: 'Demo' }
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat onOpenGitChanges={vi.fn()}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    expect(screen.getByLabelText('Git changes')).toBeDisabled()
  })

  it('navigates to /snapshots from the drawer', () => {
    tauriRef.current = false
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    fireEvent.click(screen.getByLabelText('Snapshots'))
    expect(mockNavigate).toHaveBeenCalledWith('/snapshots')
  })

  it('hides the Git history button in Tauri (desktop) mode', () => {
    tauriRef.current = true
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat onOpenGitHistory={vi.fn()}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )
    // Desktop never shows the mobile Git History entry (the ActivityRail owns
    // it there). The overflow item must not leak into the Tauri-narrow shell.
    openOverflow()
    expect(screen.queryByLabelText('Git history')).not.toBeInTheDocument()
  })

  it('mounts the Git history trigger in web mode and invokes onOpenGitHistory', () => {
    tauriRef.current = false
    const onOpenGitHistory = vi.fn()
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat onOpenGitHistory={onOpenGitHistory}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    const btn = screen.getByLabelText('Git history')
    expect(btn).not.toBeDisabled()
    fireEvent.click(btn)
    expect(onOpenGitHistory).toHaveBeenCalledTimes(1)
  })

  it('disables the Git history button when no active project path', () => {
    tauriRef.current = false
    projectRef.current = { id: 'p1', name: 'Demo' }
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat onOpenGitHistory={vi.fn()}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    openOverflow()
    expect(screen.getByLabelText('Git history')).toBeDisabled()
  })
  it('lists hidden live Conversation terminals as reopenable and separates close from terminate', () => {
    useTerminalStore.setState({
      terminals: [
        {
          id: 'terminal-hidden',
          conversationId: '018f7a1c-1b4d-7c8a-9f01-0123456789ab',
          projectId: 'p1',
          name: 'Hidden live terminal',
          shell: 'bash',
          ptyId: 'pty-hidden',
          claim: 'memory-only',
          viewState: 'hidden',
          healthStatus: 'running'
        }
      ],
      ptyIdIndex: new Map([['pty-hidden', 'terminal-hidden']])
    })
    const onCloseTerminal = vi.fn()
    const onTerminateTerminal = vi.fn()
    render(
      <MemoryRouter>
        <MobileChatShell
          onNewChat={vi.fn()}
          canNewChat
          onCloseTerminal={onCloseTerminal}
          onTerminateTerminal={onTerminateTerminal}
        >
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    fireEvent.click(screen.getByLabelText('Open menu'))
    const reopen = screen.getByRole('button', { name: /Hidden live terminal.*Reopen/i })
    expect(reopen).toHaveClass('h-11')
    fireEvent.click(reopen)
    expect(mockReopenTerminalView).toHaveBeenCalledWith('terminal-hidden')
    expect(onCloseTerminal).not.toHaveBeenCalled()

    fireEvent.click(screen.getByLabelText('Open menu'))
    const terminate = screen.getByLabelText('Terminate terminal process')
    expect(terminate).toHaveClass('size-11')
    fireEvent.click(terminate)
    expect(onTerminateTerminal).toHaveBeenCalledWith('terminal-hidden', undefined)
    expect(onCloseTerminal).not.toHaveBeenCalled()
  })

  it('shows a terminal empty state and does not create a conversation', () => {
    const onNewTerminal = vi.fn()
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat onNewTerminal={onNewTerminal}>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    fireEvent.click(screen.getByRole('tab', { name: 'Terminal' }))
    expect(screen.getByText('No live terminals')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'New terminal' }))
    expect(onNewTerminal).toHaveBeenCalledTimes(1)
    expect(mockSetActiveTab).not.toHaveBeenCalled()
    expect(mockLogFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({
        source: 'mobile-shell.tab',
        message: 'from=chat to=terminal'
      })
    )
  })

  it('activates an existing terminal tab when switching to Terminal', () => {
    workspaceRef.panes = [
      {
        type: 'leaf',
        id: 'pane-1',
        tabs: [
          { type: 'agent-chat', id: 'tab-1', sessionId: 's1' },
          { type: 'terminal', id: 'tab-term', terminalId: 't1' }
        ],
        activeTabId: 'tab-1'
      }
    ]
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    fireEvent.click(screen.getByRole('tab', { name: 'Terminal' }))
    expect(mockSetActiveTab).toHaveBeenCalledWith('pane-1', 'tab-term')
    expect(document.querySelector('[data-mobile-workspace-surface="pane"]')).not.toHaveClass(
      'hidden'
    )
  })

  it('activates an existing agent-chat tab when leaving Terminal for Chat', () => {
    workspaceRef.panes = [
      {
        type: 'leaf',
        id: 'pane-1',
        tabs: [
          { type: 'agent-chat', id: 'tab-1', sessionId: 's1' },
          { type: 'terminal', id: 'tab-term', terminalId: 't1' }
        ],
        activeTabId: 'tab-term'
      }
    ]
    render(
      <MemoryRouter>
        <MobileChatShell onNewChat={vi.fn()} canNewChat>
          <div>chat body</div>
        </MobileChatShell>
      </MemoryRouter>
    )

    fireEvent.click(screen.getByRole('tab', { name: 'Terminal' }))
    mockSetActiveTab.mockClear()
    fireEvent.click(screen.getByRole('tab', { name: 'Chat' }))
    expect(mockSetActiveTab).toHaveBeenCalledWith('pane-1', 'tab-1')
    expect(mockLogFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({
        source: 'mobile-shell.tab',
        message: 'from=terminal to=chat'
      })
    )
  })
})
