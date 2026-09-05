import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useConversationStore } from '@/stores/conversation-store'
import { useFileExplorerStore } from '@/stores/file-explorer-store'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import type { Project } from '@/types/project'
import { ProjectSidebar } from './ProjectSidebar'

const {
  mockGetAvailableShells,
  mockUseProjectsWithActivity,
  mockUseProjectsWithErrors,
  mockUseProjectsWithActiveAgentChat
} = vi.hoisted(() => ({
  mockGetAvailableShells: vi.fn(),
  mockUseProjectsWithActivity: vi.fn(),
  mockUseProjectsWithErrors: vi.fn(),
  mockUseProjectsWithActiveAgentChat: vi.fn()
}))

const { mockClipboardWriteText } = vi.hoisted(() => ({
  mockClipboardWriteText: vi.fn().mockResolvedValue({ success: true })
}))

vi.mock('@/lib/api', () => ({
  shellApi: {
    getAvailableShells: mockGetAvailableShells
  },
  worktreeApi: {
    list: vi.fn().mockResolvedValue({ success: true, data: [] }),
    checkDirty: vi.fn().mockResolvedValue({
      success: true,
      data: { modified: 0, staged: 0, untracked: 0, hasChanges: false }
    }),
    ensureSymlinks: vi.fn().mockResolvedValue({ success: true, data: [] }),
    remove: vi.fn().mockResolvedValue({ success: true })
  },
  clipboardApi: {
    writeText: (text: string) => mockClipboardWriteText(text)
  }
}))

vi.mock('@/stores/terminal-store', async () => {
  const actual = await vi.importActual('@/stores/terminal-store')
  return {
    ...actual,
    useProjectsWithActivity: () => mockUseProjectsWithActivity(),
    useProjectsWithErrors: () => mockUseProjectsWithErrors()
  }
})

vi.mock('@/stores/acp-store', async () => {
  const actual = await vi.importActual('@/stores/acp-store')
  return {
    ...actual,
    useProjectsWithActiveAgentChat: () => mockUseProjectsWithActiveAgentChat()
  }
})

vi.mock('@/lib/utils', async () => {
  const actual = await vi.importActual('@/lib/utils')
  return { ...actual }
})

// Stub the Radix context-menu primitives. The real primitives render via a
// portal + Radix positioning + pointer-based `onSelect` that is hard to drive
// from jsdom; this stub models the open/submenu/radio state in plain DOM so
// the existing menu tests (open on right-click, click wiring, submenu hover,
// Escape close, capability-gated items) assert the gating logic without the
// Radix portal/pointer plumbing. Mirrors the FileTreeContextMenu /
// GlobalContextMenu stub patterns.
vi.mock('@/components/ui/context-menu', async () => {
  const React = await import('react')
  const MenuCtx = React.createContext<{ open: boolean; setOpen: (o: boolean) => void }>({
    open: false,
    setOpen: () => {}
  })
  const SubCtx = React.createContext<{ subOpen: boolean; setSubOpen: (o: boolean) => void }>({
    subOpen: false,
    setSubOpen: () => {}
  })
  const RadioCtx = React.createContext<{ value: string; onValueChange: (v: string) => void }>({
    value: '',
    onValueChange: () => {}
  })

  const ContextMenu = ({ children }: { children: React.ReactNode }) => {
    const [open, setOpen] = React.useState(false)
    React.useEffect(() => {
      if (!open) return
      const onKey = (e: KeyboardEvent) => {
        if (e.key === 'Escape') setOpen(false)
      }
      document.addEventListener('keydown', onKey)
      return () => document.removeEventListener('keydown', onKey)
    }, [open])
    return <MenuCtx.Provider value={{ open, setOpen }}>{children}</MenuCtx.Provider>
  }
  const ContextMenuTrigger = ({
    children,
    asChild
  }: {
    children: React.ReactNode
    asChild?: boolean
  }) => {
    const { setOpen } = React.useContext(MenuCtx)
    const merged = (e: React.MouseEvent) => {
      // F2: mirror Radix's composeEventHandlers({ checkForDefaultPrevented: true }) —
      // the child's onContextMenu runs first; if it called preventDefault, do NOT
      // open. This makes the stub catch F1-type regressions (a handler that
      // re-introduces preventDefault would suppress the menu open).
      if (e.defaultPrevented) return
      e.preventDefault()
      setOpen(true)
    }
    if (asChild && React.isValidElement(children)) {
      const child = children as React.ReactElement<{
        onContextMenu?: (e: React.MouseEvent) => void
      }>
      return React.cloneElement(child, {
        onContextMenu: (e: React.MouseEvent) => {
          child.props.onContextMenu?.(e)
          merged(e)
        }
      })
    }
    return <div onContextMenu={merged}>{children}</div>
  }
  const ContextMenuContent = ({ children }: { children: React.ReactNode }) => {
    const { open } = React.useContext(MenuCtx)
    if (!open) return null
    return <div role="menu">{children}</div>
  }
  const ContextMenuItem = ({
    children,
    disabled,
    onSelect,
    variant
  }: {
    children: React.ReactNode
    disabled?: boolean
    onSelect?: () => void
    variant?: 'default' | 'destructive'
  }) => (
    <div
      role="menuitem"
      data-disabled={disabled ? '' : undefined}
      data-variant={variant}
      onClick={() => {
        if (!disabled) onSelect?.()
      }}
    >
      {children}
    </div>
  )
  const ContextMenuSeparator = () => <hr />
  const ContextMenuSub = ({ children }: { children: React.ReactNode }) => {
    const [subOpen, setSubOpen] = React.useState(false)
    return <SubCtx.Provider value={{ subOpen, setSubOpen }}>{children}</SubCtx.Provider>
  }
  const ContextMenuSubTrigger = ({ children }: { children: React.ReactNode }) => {
    const { setSubOpen } = React.useContext(SubCtx)
    return (
      <div role="menuitem" onMouseEnter={() => setSubOpen(true)}>
        {children}
      </div>
    )
  }
  const ContextMenuSubContent = ({ children }: { children: React.ReactNode }) => {
    const { subOpen } = React.useContext(SubCtx)
    if (!subOpen) return null
    return <div>{children}</div>
  }
  const ContextMenuRadioGroup = ({
    children,
    value,
    onValueChange
  }: {
    children: React.ReactNode
    value: string
    onValueChange: (v: string) => void
  }) => <RadioCtx.Provider value={{ value, onValueChange }}>{children}</RadioCtx.Provider>
  const ContextMenuRadioItem = ({
    children,
    value
  }: {
    children: React.ReactNode
    value: string
  }) => {
    const { onValueChange } = React.useContext(RadioCtx)
    return (
      <div role="menuitemradio" onClick={() => onValueChange(value)}>
        {children}
      </div>
    )
  }
  const ContextMenuCheckboxItem = ({
    children,
    checked,
    onSelect
  }: {
    children: React.ReactNode
    checked?: boolean
    onSelect?: () => void
  }) => (
    <div
      role="menuitemcheckbox"
      data-checked={checked ? '' : undefined}
      onClick={() => onSelect?.()}
    >
      {children}
    </div>
  )
  return {
    ContextMenu,
    ContextMenuTrigger,
    ContextMenuContent,
    ContextMenuItem,
    ContextMenuSeparator,
    ContextMenuSub,
    ContextMenuSubTrigger,
    ContextMenuSubContent,
    ContextMenuRadioGroup,
    ContextMenuRadioItem,
    ContextMenuCheckboxItem
  }
})

// Setup mock data
beforeEach(() => {
  useConversationStore.getState().reset()
  useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
  useProjectStore.setState({ groups: [], activeGroupId: null })
  mockGetAvailableShells.mockReset()
  mockGetAvailableShells.mockResolvedValue({
    success: true,
    data: {
      default: { path: '/bin/bash', name: 'bash', displayName: 'Bash' },
      available: [
        { path: '/bin/bash', name: 'bash', displayName: 'Bash' },
        { path: '/usr/bin/zsh', name: 'zsh', displayName: 'Zsh' },
        { path: '/bin/sh', name: 'sh', displayName: 'Shell' }
      ]
    }
  })
  mockUseProjectsWithActivity.mockReset()
  mockUseProjectsWithActivity.mockReturnValue([])
  mockUseProjectsWithActiveAgentChat.mockReset()
  mockUseProjectsWithActiveAgentChat.mockReturnValue([])
  mockUseProjectsWithErrors.mockReset()
  mockUseProjectsWithErrors.mockReturnValue(new Set())
  mockClipboardWriteText.mockClear()
})

const mockProjects: Project[] = [
  { id: '1', name: 'Project One', color: 'blue', gitBranch: 'main' },
  { id: '2', name: 'Project Two', color: 'green', gitBranch: 'develop' }
]

const defaultProps = {
  projects: mockProjects,
  activeProjectId: '1',
  onSelectProject: vi.fn(),
  onOpenProjectTerminal: vi.fn(),
  onNewProject: vi.fn(),
  onUpdateProject: vi.fn(),
  onDeleteProject: vi.fn(),
  onArchiveProject: vi.fn(),
  onRestoreProject: vi.fn(),
  onReorderProjects: vi.fn()
}

function LocationProbe(): React.JSX.Element {
  const location = useLocation()
  return <output data-testid="location-probe">{location.pathname}</output>
}

const renderWithRouter = (props = {}) => {
  return render(
    <MemoryRouter>
      <ProjectSidebar {...defaultProps} {...props} />
    </MemoryRouter>
  )
}

describe('ProjectSidebar Context Menu', () => {
  it('should open context menu on right-click', () => {
    renderWithRouter()

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)

    expect(screen.getByText('Rename')).toBeInTheDocument()
    expect(screen.getByText('Change Color')).toBeInTheDocument()
    expect(screen.getByText('Archive')).toBeInTheDocument()
    expect(screen.getByText('Delete')).toBeInTheDocument()
  })

  it('copies the project path from the context menu', () => {
    renderWithRouter({
      projects: [{ id: '1', name: 'Project One', color: 'blue', path: '/Users/dev/work/alpha' }]
    })

    fireEvent.contextMenu(screen.getByText('Project One'))
    fireEvent.click(screen.getByText('Copy path'))

    expect(mockClipboardWriteText).toHaveBeenCalledWith('/Users/dev/work/alpha')
  })

  it('disables the copy entry for a project that has no folder', () => {
    // Nothing to copy. Asserting the disabled state, not just "nothing was
    // copied": the handler guards on `project.path` too, so the behavioural
    // assertion alone passes with or without `disabled` and proves nothing
    // about the affordance the user actually sees.
    renderWithRouter({ projects: [{ id: '1', name: 'Project One', color: 'blue' }] })

    fireEvent.contextMenu(screen.getByText('Project One'))
    const entry = screen.getByText('Copy path').closest('[role="menuitem"]')

    expect(entry).toHaveAttribute('data-disabled')

    fireEvent.click(screen.getByText('Copy path'))
    expect(mockClipboardWriteText).not.toHaveBeenCalled()
  })

  it('leaves the copy entry enabled when the project has a folder', () => {
    renderWithRouter({
      projects: [{ id: '1', name: 'Project One', color: 'blue', path: '/tmp/alpha' }]
    })

    fireEvent.contextMenu(screen.getByText('Project One'))

    expect(screen.getByText('Copy path').closest('[role="menuitem"]')).not.toHaveAttribute(
      'data-disabled'
    )
  })

  it('should close context menu on escape', async () => {
    renderWithRouter()

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)

    expect(screen.getByText('Rename')).toBeInTheDocument()

    fireEvent.keyDown(document, { key: 'Escape' })

    await waitFor(() => {
      expect(screen.queryByText('Rename')).not.toBeInTheDocument()
    })
  })

  it('should start inline editing when Rename is clicked', async () => {
    renderWithRouter()

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)

    fireEvent.click(screen.getByText('Rename'))

    await waitFor(() => {
      const input = screen.getByRole('textbox')
      expect(input).toBeInTheDocument()
      expect(input).toHaveValue('Project One')
    })
  })

  it('should save rename on Enter key', async () => {
    const onUpdateProject = vi.fn()
    renderWithRouter({ onUpdateProject })

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)
    fireEvent.click(screen.getByText('Rename'))

    const input = await screen.findByRole('textbox')
    fireEvent.change(input, { target: { value: 'New Project Name' } })
    fireEvent.keyDown(input, { key: 'Enter' })

    expect(onUpdateProject).toHaveBeenCalledWith('1', { name: 'New Project Name' })
  })

  it('should cancel rename on Escape key', async () => {
    const onUpdateProject = vi.fn()
    renderWithRouter({ onUpdateProject })

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)
    fireEvent.click(screen.getByText('Rename'))

    const input = await screen.findByRole('textbox')
    fireEvent.change(input, { target: { value: 'New Name' } })
    fireEvent.keyDown(input, { key: 'Escape' })

    await waitFor(() => {
      expect(screen.queryByRole('textbox')).not.toBeInTheDocument()
    })
    expect(onUpdateProject).not.toHaveBeenCalled()
  })

  it('should call onArchiveProject when Archive is clicked', () => {
    const onArchiveProject = vi.fn()
    renderWithRouter({ onArchiveProject })

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)
    fireEvent.click(screen.getByText('Archive'))

    expect(onArchiveProject).toHaveBeenCalledWith('1')
  })

  it('should show delete confirmation dialog when Delete is clicked', async () => {
    renderWithRouter()

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)
    fireEvent.click(screen.getByText('Delete'))

    await waitFor(() => {
      expect(screen.getByText('Delete Project')).toBeInTheDocument()
      expect(screen.getByText(/Are you sure you want to delete/)).toBeInTheDocument()
    })
  })

  it('should call onDeleteProject when delete is confirmed', async () => {
    const onDeleteProject = vi.fn()
    renderWithRouter({ onDeleteProject })

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)
    fireEvent.click(screen.getByText('Delete'))

    await waitFor(() => {
      expect(screen.getByText('Delete Project')).toBeInTheDocument()
    })

    // Click the Delete button in the confirmation dialog
    const confirmButtons = screen.getAllByText('Delete')
    const confirmButton = confirmButtons[confirmButtons.length - 1]
    fireEvent.click(confirmButton)

    expect(onDeleteProject).toHaveBeenCalledWith('1')
  })

  it('should close delete dialog when cancelled', async () => {
    const onDeleteProject = vi.fn()
    renderWithRouter({ onDeleteProject })

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)
    fireEvent.click(screen.getByText('Delete'))

    await waitFor(() => {
      expect(screen.getByText('Delete Project')).toBeInTheDocument()
    })

    fireEvent.click(screen.getByText('Cancel'))

    await waitFor(() => {
      expect(screen.queryByText('Delete Project')).not.toBeInTheDocument()
    })
    expect(onDeleteProject).not.toHaveBeenCalled()
  })

  it('should open color picker when Change Color is clicked', async () => {
    renderWithRouter()

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)
    fireEvent.click(screen.getByText('Change Color'))

    await waitFor(() => {
      expect(screen.getByText('Select Color')).toBeInTheDocument()
    })
  })
})

// F1/F2 regression guards: assert the menu OPENS on right-click for each
// surface. The F2 stub (checkForDefaultPrevented) skips open if the child
// handler calls preventDefault — so if F1's preventDefault removal is ever
// reverted in handleContextMenu / handleGroupContextMenu, these tests fail
// (the menu items vanish because Radix's open step is skipped).
describe('ProjectSidebar context menu open regression (F1/F2)', () => {
  beforeEach(() => {
    useProjectStore.setState({ groups: [] })
  })

  it('project row menu opens on right-click', () => {
    renderWithRouter()

    fireEvent.contextMenu(screen.getByText('Project One'))

    expect(screen.getByText('Rename')).toBeInTheDocument()
    expect(screen.getByText('Archive')).toBeInTheDocument()
    expect(screen.getByText('Delete')).toBeInTheDocument()
  })

  it('group header menu opens on right-click', () => {
    useProjectStore.setState({
      groups: [{ id: 'group-1', name: 'My Folder', projectIds: ['1'], isCollapsed: false }]
    })
    renderWithRouter()

    fireEvent.contextMenu(screen.getByText('My Folder'))

    expect(screen.getByText('Rename Group')).toBeInTheDocument()
    expect(screen.getByText('Change Color')).toBeInTheDocument()
    expect(screen.getByText('Delete Group (Keep Projects)')).toBeInTheDocument()
  })

  it('archived project menu opens on right-click', async () => {
    const archived: Project[] = [
      { id: '1', name: 'Active Project', color: 'blue', gitBranch: 'main' },
      {
        id: '2',
        name: 'Archived Project',
        color: 'green',
        gitBranch: 'develop',
        isArchived: true
      }
    ]
    renderWithRouter({ projects: archived })

    fireEvent.click(screen.getByText(/Archived \(1\)/))
    await waitFor(() => expect(screen.getByText('Archived Project')).toBeInTheDocument())

    fireEvent.contextMenu(screen.getByText('Archived Project'))

    expect(screen.getByText('Restore')).toBeInTheDocument()
    expect(screen.getByText('Delete')).toBeInTheDocument()
  })
})

describe('ProjectSidebar', () => {
  it('should render project list', () => {
    renderWithRouter()

    expect(screen.getByText('Project One')).toBeInTheDocument()
    expect(screen.getByText('Project Two')).toBeInTheDocument()
  })

  it('keeps projects as context only and does not nest Conversation navigation', () => {
    renderWithRouter()

    expect(screen.queryByLabelText('Search conversations')).not.toBeInTheDocument()
    expect(screen.queryByText('Refactor sidebar')).not.toBeInTheDocument()
  })

  it('should call onSelectProject when project is clicked', () => {
    const onSelectProject = vi.fn()
    renderWithRouter({ onSelectProject })

    fireEvent.click(screen.getByText('Project Two'))

    expect(onSelectProject).toHaveBeenCalledWith('2')
  })

  it('opens a terminal from the row button without selecting through the row click', () => {
    // Selecting a project now means "show me this folder"; a shell in it is a
    // separate, explicit request. Both used to be the same click.
    const onSelectProject = vi.fn()
    const onOpenProjectTerminal = vi.fn()
    renderWithRouter({ onSelectProject, onOpenProjectTerminal })

    fireEvent.click(screen.getByRole('button', { name: /Open a terminal in Project Two/i }))

    expect(onOpenProjectTerminal).toHaveBeenCalledWith('2')
    // The button stops propagation, so the row's own select must not fire.
    expect(onSelectProject).not.toHaveBeenCalled()
  })

  it('gives every non-archived project row its own terminal button', () => {
    renderWithRouter()

    expect(
      screen.getByRole('button', { name: /Open a terminal in Project One/i })
    ).toBeInTheDocument()
    expect(
      screen.getByRole('button', { name: /Open a terminal in Project Two/i })
    ).toBeInTheDocument()
  })

  it('keeps the canonical route, Conversation workspace, and PTY records on project switch', () => {
    const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'
    useProjectStore.setState({ projects: mockProjects, activeProjectId: '1' })
    useConversationStore.setState({ activeConversationId: conversationId })
    useWorkspaceStore.getState().addAgentChatTab('opaque-runtime-session', undefined, false)
    const rootBefore = useWorkspaceStore.getState().root
    useTerminalStore.setState({
      terminals: [
        {
          id: 'terminal-live',
          conversationId,
          projectId: '1',
          name: 'Live',
          shell: 'bash',
          ptyId: 'pty-live',
          healthStatus: 'running',
          viewState: 'visible',
          isHidden: false,
          output: []
        }
      ]
    })

    render(
      <MemoryRouter initialEntries={[`/c/${conversationId}`]}>
        <ProjectSidebar
          {...defaultProps}
          onSelectProject={(id) => useProjectStore.getState().selectProject(id)}
        />
        <LocationProbe />
      </MemoryRouter>
    )
    fireEvent.click(screen.getByText('Project Two'))

    expect(screen.getByTestId('location-probe')).toHaveTextContent(`/c/${conversationId}`)
    expect(useConversationStore.getState().activeConversationId).toBe(conversationId)
    expect(useWorkspaceStore.getState().root).toBe(rootBefore)
    expect(JSON.stringify(useWorkspaceStore.getState().root)).toContain('opaque-runtime-session')
    expect(useTerminalStore.getState().terminals[0].ptyId).toBe('pty-live')
  })

  it('should call onNewProject when header + button is clicked', () => {
    const onNewProject = vi.fn()
    renderWithRouter({ onNewProject })

    // Use data-testid for robust button selection
    const headerButton = screen.getByTestId('header-new-project')
    fireEvent.click(headerButton)

    expect(onNewProject).toHaveBeenCalled()
  })

  it('should show version label at the bottom', () => {
    renderWithRouter({})

    expect(screen.getByText(/Se Manager/)).toBeInTheDocument()
  })

  it('should show empty state when no projects', () => {
    renderWithRouter({ projects: [] })

    expect(screen.getByText('No projects yet')).toBeInTheDocument()
  })

  it('should not render removed navigation items', () => {
    renderWithRouter()

    // These items were removed from the sidebar
    expect(screen.queryByText('Workspace')).not.toBeInTheDocument()
    expect(screen.queryByText('Snapshots')).not.toBeInTheDocument()
    expect(screen.queryByText('Settings')).not.toBeInTheDocument()
    expect(screen.queryByText('Preferences')).not.toBeInTheDocument()
  })

  it('should not render removed action items', () => {
    renderWithRouter()

    // These actions were removed from the sidebar
    expect(screen.queryByText('Scan Directories')).not.toBeInTheDocument()
    expect(screen.queryByText('Import Config')).not.toBeInTheDocument()
  })

  it('should handle project with empty name gracefully', () => {
    const projectsWithEmptyName: Project[] = [
      { id: '1', name: '', color: 'blue', gitBranch: 'main' }
    ]
    renderWithRouter({ projects: projectsWithEmptyName })

    // Empty-named project still renders its row without crashing.
    expect(screen.getByTestId('active-projects-container')).toBeInTheDocument()
  })
})

describe('ProjectSidebar Name Truncation', () => {
  const longName =
    'A Very Long Project Name That Would Otherwise Wrap Onto A Second Line In The Narrow Sidebar'

  it('truncates a long active project name instead of wrapping', () => {
    renderWithRouter({
      projects: [{ id: '1', name: longName, color: 'blue', gitBranch: 'main' }],
      activeProjectId: '1'
    })

    const nameEl = screen.getByText(longName)
    // truncate => overflow-hidden + text-ellipsis + whitespace-nowrap;
    // min-w-0 lets the flex child shrink below its content width so clipping kicks in.
    expect(nameEl).toHaveClass('truncate', 'min-w-0', 'flex-1')
    // Full name remains discoverable on hover.
    expect(nameEl).toHaveAttribute('title', longName)
  })

  it('truncates a long archived project name and exposes the full name via title', () => {
    renderWithRouter({
      projects: [
        { id: '1', name: 'Active Project', color: 'blue', gitBranch: 'main' },
        { id: '2', name: longName, color: 'green', gitBranch: 'develop', isArchived: true }
      ]
    })

    // Expand the archived section.
    fireEvent.click(screen.getByText(/Archived \(1\)/))

    const nameEl = screen.getByText(longName)
    expect(nameEl).toHaveClass('truncate', 'min-w-0', 'flex-1')
    expect(nameEl).toHaveAttribute('title', longName)
  })
})

describe('ProjectSidebar Archived Projects', () => {
  const projectsWithArchived: Project[] = [
    { id: '1', name: 'Active Project', color: 'blue', gitBranch: 'main' },
    { id: '2', name: 'Archived Project', color: 'green', gitBranch: 'develop', isArchived: true }
  ]

  it('should show archived section toggle when there are archived projects', () => {
    renderWithRouter({ projects: projectsWithArchived })

    expect(screen.getByText(/Archived \(1\)/)).toBeInTheDocument()
  })

  it('should not show archived projects by default', () => {
    renderWithRouter({ projects: projectsWithArchived })

    expect(screen.getByText('Active Project')).toBeInTheDocument()
    expect(screen.queryByText('Archived Project')).not.toBeInTheDocument()
  })

  it('should show archived projects when toggle is clicked', async () => {
    renderWithRouter({ projects: projectsWithArchived })

    fireEvent.click(screen.getByText(/Archived \(1\)/))

    await waitFor(() => {
      expect(screen.getByText('Archived Project')).toBeInTheDocument()
    })
  })

  it('should show Restore option in context menu for archived projects', async () => {
    renderWithRouter({ projects: projectsWithArchived })

    // Expand archived section
    fireEvent.click(screen.getByText(/Archived \(1\)/))

    await waitFor(() => {
      expect(screen.getByText('Archived Project')).toBeInTheDocument()
    })

    // Right-click on archived project
    fireEvent.contextMenu(screen.getByText('Archived Project'))

    expect(screen.getByText('Restore')).toBeInTheDocument()
    expect(screen.queryByText('Rename')).not.toBeInTheDocument()
    expect(screen.queryByText('Archive')).not.toBeInTheDocument()
  })

  it('should call onRestoreProject when Restore is clicked', async () => {
    const onRestoreProject = vi.fn()
    renderWithRouter({ projects: projectsWithArchived, onRestoreProject })

    // Expand archived section
    fireEvent.click(screen.getByText(/Archived \(1\)/))

    await waitFor(() => {
      expect(screen.getByText('Archived Project')).toBeInTheDocument()
    })

    // Right-click on archived project and click Restore
    fireEvent.contextMenu(screen.getByText('Archived Project'))
    fireEvent.click(screen.getByText('Restore'))

    expect(onRestoreProject).toHaveBeenCalledWith('2')
  })

  it('should not show archived section when there are no archived projects', () => {
    renderWithRouter({ projects: mockProjects })

    expect(screen.queryByText(/Archived/)).not.toBeInTheDocument()
  })

  it('does not apply extra opacity to an archived project activity spinner', () => {
    mockUseProjectsWithActivity.mockReturnValue(['2'])
    renderWithRouter({ projects: projectsWithArchived })
    fireEvent.click(screen.getByText(/Archived \(1\)/))

    const row = screen.getByTestId('archived-project-item-2')
    const spinner = screen.getByRole('status', { name: 'Project activity' })
    expect(row).toHaveClass('opacity-60')
    expect(spinner).not.toHaveClass('opacity-60')
  })
})

describe('ProjectSidebar Default Shell Submenu', () => {
  it('should show Set Default Shell menu item with submenu', async () => {
    renderWithRouter()

    // Wait for shells to be fetched
    await waitFor(() => {
      expect(mockGetAvailableShells).toHaveBeenCalled()
    })

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)

    await waitFor(() => {
      expect(screen.getByText('Default Shell')).toBeInTheDocument()
    })
  })

  it('should call onUpdateProject when shell is selected from submenu', async () => {
    const onUpdateProject = vi.fn()
    renderWithRouter({ onUpdateProject })

    // Wait for shells to be fetched
    await waitFor(() => {
      expect(mockGetAvailableShells).toHaveBeenCalled()
    })

    const projectItem = screen.getByText('Project One')
    fireEvent.contextMenu(projectItem)

    // Hover over Default Shell to show submenu
    const shellMenuItem = await screen.findByText('Default Shell')
    fireEvent.mouseEnter(shellMenuItem.closest('div')!)

    // Click on Zsh in submenu
    await waitFor(() => {
      expect(screen.getByText('Zsh')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByText('Zsh'))

    expect(onUpdateProject).toHaveBeenCalledWith('1', { defaultShell: '/usr/bin/zsh' })
  })
})

describe('ProjectSidebar Activity Indicator', () => {
  function activityIndicator(item: HTMLElement): HTMLElement | null {
    return item.querySelector('[title="Activity"]')
  }

  beforeEach(() => {
    mockUseProjectsWithActiveAgentChat.mockReturnValue([])
  })

  it('should not show activity indicator when hasActivity is false', () => {
    mockUseProjectsWithActivity.mockReturnValue([])
    renderWithRouter()

    const item = screen.getByTestId('project-item-1')
    expect(activityIndicator(item)).toBeNull()
  })

  it('should show activity indicator when terminal activity is true', () => {
    mockUseProjectsWithActivity.mockReturnValue(['2'])
    renderWithRouter()

    const item = screen.getByTestId('project-item-2')
    expect(activityIndicator(item)).not.toBeNull()
    expect(activityIndicator(item)).toHaveAttribute('title', 'Activity')
    expect(screen.getByRole('status', { name: 'Project activity' })).toBeInTheDocument()
  })

  it('should show activity indicator when agent chat is active', () => {
    mockUseProjectsWithActivity.mockReturnValue([])
    mockUseProjectsWithActiveAgentChat.mockReturnValue(['2'])
    renderWithRouter()

    const item = screen.getByTestId('project-item-2')
    expect(activityIndicator(item)).not.toBeNull()
    expect(screen.getByRole('status', { name: 'Project activity' })).toBeInTheDocument()
  })

  it('should show activity indicator even when project is active if hasActivity is true', () => {
    mockUseProjectsWithActivity.mockReturnValue(['1'])
    renderWithRouter()

    const item = screen.getByTestId('project-item-1')
    expect(activityIndicator(item)).not.toBeNull()
  })
})

describe('ProjectSidebar Project Search', () => {
  // 8 projects crosses the PROJECT_SEARCH_THRESHOLD so the search UI renders.
  const manyProjects: Project[] = Array.from({ length: 8 }, (_, i) => ({
    id: String(i + 1),
    name: `Project ${i + 1}`,
    color: 'blue' as const,
    gitBranch: i === 7 ? 'feature/special' : 'main'
  }))

  const fewProjects: Project[] = [
    { id: '1', name: 'Alpha', color: 'blue', gitBranch: 'main' },
    { id: '2', name: 'Beta', color: 'green', gitBranch: 'main' }
  ]

  it('hides the search box when the project count is below the threshold', () => {
    renderWithRouter({ projects: fewProjects, activeProjectId: '1' })
    expect(screen.queryByTestId('project-search-input')).not.toBeInTheDocument()
  })

  it('shows the search box once the project count reaches the threshold', () => {
    renderWithRouter({ projects: manyProjects, activeProjectId: '1' })
    expect(screen.getByTestId('project-search-input')).toBeInTheDocument()
  })

  it('renders the search field flush with the rail header', () => {
    renderWithRouter({ projects: manyProjects, activeProjectId: '1' })
    expect(screen.getByTestId('project-search-input')).toHaveClass(
      'h-8',
      'bg-transparent',
      'rounded-none'
    )
  })

  it('filters the visible projects by name', () => {
    renderWithRouter({ projects: manyProjects, activeProjectId: '1' })

    fireEvent.change(screen.getByTestId('project-search-input'), {
      target: { value: 'Project 8' }
    })

    expect(screen.getByText('Project 8')).toBeInTheDocument()
    expect(screen.queryByText('Project 1')).not.toBeInTheDocument()
    expect(screen.queryByText('Project 2')).not.toBeInTheDocument()
  })

  it('matches on git branch as well as name', () => {
    renderWithRouter({ projects: manyProjects, activeProjectId: '1' })

    fireEvent.change(screen.getByTestId('project-search-input'), {
      target: { value: 'feature/special' }
    })

    expect(screen.getByText('Project 8')).toBeInTheDocument()
    expect(screen.queryByText('Project 1')).not.toBeInTheDocument()
  })

  it('shows an empty state when nothing matches', () => {
    renderWithRouter({ projects: manyProjects, activeProjectId: '1' })

    fireEvent.change(screen.getByTestId('project-search-input'), {
      target: { value: 'no-such-project' }
    })

    expect(screen.getByTestId('project-search-empty')).toBeInTheDocument()
    expect(screen.getByText('No projects found')).toBeInTheDocument()
  })

  it('clears the query when the clear button is clicked', () => {
    renderWithRouter({ projects: manyProjects, activeProjectId: '1' })

    const input = screen.getByTestId('project-search-input') as HTMLInputElement
    fireEvent.change(input, { target: { value: 'Project 8' } })
    expect(screen.queryByText('Project 1')).not.toBeInTheDocument()

    fireEvent.click(screen.getByTestId('project-search-clear'))

    expect(input.value).toBe('')
    expect(screen.getByText('Project 1')).toBeInTheDocument()
  })

  it('clears the query when Escape is pressed in the search box', () => {
    renderWithRouter({ projects: manyProjects, activeProjectId: '1' })

    const input = screen.getByTestId('project-search-input') as HTMLInputElement
    fireEvent.change(input, { target: { value: 'Project 8' } })
    fireEvent.keyDown(input, { key: 'Escape' })

    expect(input.value).toBe('')
    expect(screen.getByText('Project 1')).toBeInTheDocument()
  })

  it('keeps the Ctrl+1 shortcut badge tied to the unfiltered position while searching', () => {
    renderWithRouter({ projects: manyProjects, activeProjectId: '1' })

    // Project 1 is index 0 -> Ctrl+1. Search for it; the badge must stay Ctrl+1.
    fireEvent.change(screen.getByTestId('project-search-input'), {
      target: { value: 'Project 1' }
    })

    expect(screen.getByText('Project 1')).toBeInTheDocument()
    expect(screen.getByText('Ctrl+1')).toBeInTheDocument()
  })

  it('clears a lingering query when the search box drops below the threshold', () => {
    const { rerender } = render(
      <MemoryRouter>
        <ProjectSidebar {...defaultProps} projects={manyProjects} activeProjectId="1" />
      </MemoryRouter>
    )

    fireEvent.change(screen.getByTestId('project-search-input'), {
      target: { value: 'Project 8' }
    })
    expect(screen.queryByText('Project 1')).not.toBeInTheDocument()

    // Drop below the threshold so the search box unmounts.
    rerender(
      <MemoryRouter>
        <ProjectSidebar {...defaultProps} projects={fewProjects} activeProjectId="1" />
      </MemoryRouter>
    )

    // No stuck filter: the search box is gone and the remaining projects are visible.
    expect(screen.queryByTestId('project-search-input')).not.toBeInTheDocument()
    expect(screen.getByText('Alpha')).toBeInTheDocument()
    expect(screen.getByText('Beta')).toBeInTheDocument()
  })

  it('clears the search when the active project is not in the filtered results', () => {
    const { rerender } = render(
      <MemoryRouter>
        <ProjectSidebar {...defaultProps} projects={manyProjects} activeProjectId="1" />
      </MemoryRouter>
    )

    // Filter to a single project that is NOT the active one.
    fireEvent.change(screen.getByTestId('project-search-input'), {
      target: { value: 'Project 8' }
    })
    expect(screen.queryByText('Project 2')).not.toBeInTheDocument()

    // Active project switches to one hidden by the query (e.g. Ctrl+2 or a new project).
    rerender(
      <MemoryRouter>
        <ProjectSidebar {...defaultProps} projects={manyProjects} activeProjectId="2" />
      </MemoryRouter>
    )

    // Search self-clears so the now-active project is visible again.
    expect((screen.getByTestId('project-search-input') as HTMLInputElement).value).toBe('')
    expect(screen.getByText('Project 2')).toBeInTheDocument()
  })

  it('disables the archived toggle while searching', () => {
    const withArchived: Project[] = [
      ...manyProjects,
      { id: '99', name: 'Old Project', color: 'gray', gitBranch: 'main', isArchived: true }
    ]
    renderWithRouter({ projects: withArchived, activeProjectId: '1' })

    fireEvent.change(screen.getByTestId('project-search-input'), {
      target: { value: 'Project' }
    })

    const toggle = screen.getByLabelText(/Archived projects/)
    expect(toggle).toBeDisabled()
  })
})

describe('ProjectSidebar Folder Grouping', () => {
  beforeEach(() => {
    // Reset groups in the store before each test
    useProjectStore.setState({ groups: [], activeGroupId: null })
  })

  it('should render active projects grouped under folder section when groups are configured', () => {
    useProjectStore.setState({
      groups: [
        {
          id: 'group-1',
          name: 'My Folder',
          projectIds: ['1'],
          isCollapsed: false
        }
      ]
    })

    renderWithRouter()

    // Folder header should be visible
    expect(screen.getByText('My Folder')).toBeInTheDocument()
    // Project One (id: 1) should be nested inside the folder
    expect(screen.getByText('Project One')).toBeInTheDocument()
  })

  it('should hide folder contents when group is collapsed', async () => {
    useProjectStore.setState({
      groups: [
        {
          id: 'group-1',
          name: 'My Folder',
          projectIds: ['1'],
          isCollapsed: true
        }
      ]
    })

    renderWithRouter()

    expect(screen.getByText('My Folder')).toBeInTheDocument()
    // Since it is collapsed, Project One should NOT be rendered
    expect(screen.queryByText('Project One')).not.toBeInTheDocument()
  })

  it('should support custom folder group colors', () => {
    useProjectStore.setState({
      groups: [
        {
          id: 'group-1',
          name: 'My Folder',
          projectIds: ['1'],
          isCollapsed: false,
          color: 'purple'
        }
      ]
    })

    renderWithRouter()

    const folderHeader = screen.getByRole('button', { name: 'My Folder' })
    const iconContainer = folderHeader.querySelector('.text-project-purple')
    expect(iconContainer).toBeInTheDocument()
  })

  it('selects a group from its header without toggling collapse', () => {
    const onSelectGroup = vi.fn()
    useProjectStore.setState({
      groups: [
        {
          id: 'group-1',
          name: 'My Folder',
          projectIds: ['1'],
          isCollapsed: false
        }
      ]
    })

    renderWithRouter({ onSelectGroup })
    fireEvent.click(screen.getByRole('button', { name: 'My Folder' }))

    expect(onSelectGroup).toHaveBeenCalledWith('group-1')
    expect(useProjectStore.getState().groups[0].isCollapsed).toBe(false)
  })

  it('uses the store selector without changing the current route', () => {
    useProjectStore.setState({
      projects: mockProjects,
      groups: [
        {
          id: 'group-1',
          name: 'My Folder',
          projectIds: ['1', '2'],
          preferredProjectId: '2',
          isCollapsed: false
        }
      ],
      activeProjectId: '1',
      activeGroupId: null
    })

    render(
      <MemoryRouter initialEntries={['/settings']}>
        <ProjectSidebar {...defaultProps} />
        <LocationProbe />
      </MemoryRouter>
    )
    fireEvent.click(screen.getByRole('button', { name: 'My Folder' }))

    expect(useProjectStore.getState().activeGroupId).toBe('group-1')
    expect(useProjectStore.getState().activeProjectId).toBe('2')
    expect(screen.getByTestId('location-probe')).toHaveTextContent('/settings')
  })

  it('toggles collapse only from the dedicated chevron control', () => {
    const onSelectGroup = vi.fn()
    useProjectStore.setState({
      groups: [
        {
          id: 'group-1',
          name: 'My Folder',
          projectIds: ['1'],
          isCollapsed: false
        }
      ]
    })

    renderWithRouter({ onSelectGroup })
    fireEvent.click(screen.getByTestId('project-group-chevron-group-1'))

    expect(useProjectStore.getState().groups[0].isCollapsed).toBe(true)
    expect(onSelectGroup).not.toHaveBeenCalled()
  })

  it('exposes and styles the active group instead of its preferred project row', () => {
    useProjectStore.setState({
      groups: [
        {
          id: 'group-1',
          name: 'My Folder',
          projectIds: ['1'],
          preferredProjectId: '1',
          isCollapsed: false
        }
      ],
      activeProjectId: '1',
      activeGroupId: 'group-1'
    })

    renderWithRouter()

    expect(screen.getByRole('button', { name: 'My Folder' })).toHaveAttribute(
      'aria-pressed',
      'true'
    )
    expect(screen.getByTestId('project-group-group-1')).toHaveClass('bg-sidebar-accent')
    expect(screen.getByTestId('project-item-1').querySelector('[aria-current]')).toBeNull()
  })
})

describe('ProjectSidebar compact rail', () => {
  it('marks the active project with a 28px row, lichen inset, and color marker', () => {
    renderWithRouter()

    const row = screen.getByTestId('project-item-1').querySelector('[aria-current="page"]')
    expect(row).toHaveClass('h-7', 'bg-sidebar-accent', 'ring-1')
    expect(row?.querySelector('[data-project-color="blue"]')).toHaveClass('size-1.5')
  })

  it('keeps header actions discoverable as compact rail icons', () => {
    renderWithRouter()

    expect(screen.getByTestId('header-new-project')).toHaveClass('size-7')
    expect(screen.getByLabelText('New Group Folder')).toHaveClass('size-7')
  })

  it('exposes the editor-import header action when a handler is provided', () => {
    const onImportFromEditor = vi.fn()
    renderWithRouter({ onImportFromEditor })
    fireEvent.click(screen.getByTestId('header-import-editors'))
    expect(onImportFromEditor).toHaveBeenCalledTimes(1)
  })
})

describe('ProjectSidebar cross-project file drop', () => {
  const withPaths: Project[] = [
    { id: '1', name: 'Project One', color: 'blue', path: '/work/one' },
    { id: '2', name: 'Project Two', color: 'green', path: '/work/two' },
    { id: '3', name: 'Pathless', color: 'red' }
  ]

  function dataTransfer(): DataTransfer {
    return { dropEffect: '', effectAllowed: '', types: [] as string[] } as unknown as DataTransfer
  }

  function row(projectId: string): HTMLElement {
    const node = screen.getByTestId(`project-item-${projectId}`).querySelector('[role="button"]')
    if (!node) throw new Error(`no row for ${projectId}`)
    return node as HTMLElement
  }

  const moveEntries = vi.fn()

  beforeEach(() => {
    moveEntries.mockClear()
    useFileExplorerStore.setState({ dragPaths: [], moveEntries })
  })

  it('should move dragged entries into the dropped project root', () => {
    useFileExplorerStore.setState({ dragPaths: ['/work/one/notes.md'] })
    renderWithRouter({ projects: withPaths })

    fireEvent.drop(row('2'), { dataTransfer: dataTransfer() })

    // A project row is the only place another project's root is addressable
    // while the tree is still showing this project.
    expect(moveEntries).toHaveBeenCalledWith(['/work/one/notes.md'], '/work/two')
  })

  it('should reject a drop back onto the project the entry already lives in', () => {
    useFileExplorerStore.setState({ dragPaths: ['/work/one/notes.md'] })
    renderWithRouter({ projects: withPaths })

    fireEvent.drop(row('1'), { dataTransfer: dataTransfer() })

    expect(moveEntries).not.toHaveBeenCalled()
  })

  it('should stay inert for a project that has no folder', () => {
    useFileExplorerStore.setState({ dragPaths: ['/work/one/notes.md'] })
    renderWithRouter({ projects: withPaths })

    fireEvent.drop(row('3'), { dataTransfer: dataTransfer() })

    expect(moveEntries).not.toHaveBeenCalled()
  })

  it('should highlight the project row while a legal drag is over it', () => {
    useFileExplorerStore.setState({ dragPaths: ['/work/one/notes.md'] })
    renderWithRouter({ projects: withPaths })

    fireEvent.dragOver(row('2'), { dataTransfer: dataTransfer() })

    expect(row('2')).toHaveClass('ring-primary')
  })

  it('should clear the drag once the drop is handled', () => {
    useFileExplorerStore.setState({ dragPaths: ['/work/one/notes.md'] })
    renderWithRouter({ projects: withPaths })

    fireEvent.drop(row('2'), { dataTransfer: dataTransfer() })

    // Otherwise every later dragover still sees a live payload and every row
    // stays lit.
    expect(useFileExplorerStore.getState().dragPaths).toEqual([])
  })
  it('should expose the project root so touch hit-testing can find it', () => {
    renderWithRouter({ projects: withPaths })

    // resolveLongPressDropTarget reads this attribute; a long-press drag onto
    // a project row has no destination without it.
    expect(row('2')).toHaveAttribute('data-project-path', '/work/two')
    expect(row('3')).not.toHaveAttribute('data-project-path')
  })
})
