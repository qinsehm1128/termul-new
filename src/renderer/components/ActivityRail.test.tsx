import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import * as appSettingsHooks from '@/hooks/use-app-settings'
import { useSSHPanelStore } from '@/stores/ssh-panel-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { ActivityRail } from './ActivityRail'

const { mockUpdatePanelVisibility, mockToastError, mockNavigate, platformState } = vi.hoisted(
  () => ({
    mockUpdatePanelVisibility: vi.fn(() => Promise.resolve()),
    mockToastError: vi.fn(),
    mockNavigate: vi.fn(),
    platformState: { isMac: false }
  })
)

vi.mock('sonner', () => ({
  toast: {
    error: mockToastError
  }
}))

vi.mock('@/lib/platform', () => ({
  get isMac() {
    return platformState.isMac
  }
}))

// Mutable: defaults to desktop so existing tests pass. Web-mode tests set
// this to false to verify the SSH rail button's disabled-with-reason gate.
const { tauriRef } = vi.hoisted(() => ({ tauriRef: { current: true as boolean } }))

vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => tauriRef.current
}))

vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom')
  return {
    ...actual,
    useNavigate: () => mockNavigate
  }
})

describe('ActivityRail', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    platformState.isMac = false
    vi.spyOn(appSettingsHooks, 'useUpdatePanelVisibility').mockReturnValue(
      mockUpdatePanelVisibility
    )
    useSSHPanelStore.setState({ isVisible: true })
    useTerminalStore.setState({ terminals: [], activeTerminalId: '', ptyIdIndex: new Map() })
  })

  function renderRail() {
    return render(
      <TooltipProvider delayDuration={0}>
        <MemoryRouter>
          <ActivityRail />
        </MemoryRouter>
      </TooltipProvider>
    )
  }

  it('does not render sidebar or file-explorer toggles (moved to titlebar)', () => {
    renderRail()

    expect(screen.queryByRole('button', { name: /sidebar/i })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /file explorer/i })).not.toBeInTheDocument()
  })

  it('navigates to preferences on click', () => {
    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Open preferences' }))

    expect(mockNavigate).toHaveBeenCalledWith('/preferences')
  })

  it('exposes the keyboard shortcuts trigger', () => {
    renderRail()

    expect(screen.getByRole('button', { name: 'Open keyboard shortcuts menu' })).toBeInTheDocument()
  })

  it('disables color themes when no toggle handler is provided', () => {
    renderRail()

    const themeButton = screen.getByRole('button', { name: 'Color themes' })
    expect(themeButton).toBeDisabled()
    expect(themeButton).toHaveAttribute('aria-disabled', 'true')
    expect(themeButton).not.toHaveAttribute('aria-pressed')
  })

  it('toggles color themes when a toggle handler is provided', () => {
    const onToggleThemePicker = vi.fn()
    render(
      <MemoryRouter>
        <ActivityRail isThemePickerOpen onToggleThemePicker={onToggleThemePicker} />
      </MemoryRouter>
    )

    const themeButton = screen.getByRole('button', { name: 'Color themes' })
    expect(themeButton).not.toBeDisabled()
    expect(themeButton).toHaveAttribute('aria-pressed', 'true')

    fireEvent.click(themeButton)

    expect(onToggleThemePicker).toHaveBeenCalledTimes(1)
  })

  it('renders the brand mark', () => {
    renderRail()

    expect(screen.getByRole('img', { name: 'Se' })).toBeInTheDocument()
  })

  it('separates project actions from the conversations workspace', () => {
    const { container } = renderRail()

    const projectActions = screen.getByRole('group', { name: 'Project workspace and tools' })
    const conversationActions = screen.getByRole('group', { name: 'Conversation workspace' })

    expect(within(projectActions).getByRole('button', { name: 'Open projects' })).toBeVisible()
    expect(
      within(projectActions).getByRole('button', { name: 'Open the terminal board' })
    ).toBeVisible()
    expect(
      within(conversationActions).getByRole('button', {
        name: 'Open the conversations area'
      })
    ).toBeVisible()
    expect(
      within(conversationActions).getByRole('button', { name: 'Open scheduled tasks' })
    ).toBeVisible()
    expect(
      within(projectActions).queryByRole('button', { name: 'Open scheduled tasks' })
    ).not.toBeInTheDocument()
    expect(
      projectActions.compareDocumentPosition(conversationActions) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    expect(
      container.querySelector('[data-activity-rail-divider="workspace-contexts"]')
    ).toBeInTheDocument()
  })

  it('provides hover labels for primary and utility actions', () => {
    const { container } = renderRail()

    for (const label of ['Projects', 'Terminals', 'Conversations', 'Preferences', 'Color themes']) {
      expect(container.querySelector(`[data-rail-tooltip="${label}"]`)).toBeInTheDocument()
    }
  })

  it('shows the localized label when an action is hovered', async () => {
    renderRail()

    const projectsButton = screen.getByRole('button', { name: 'Open projects' })
    fireEvent.pointerMove(projectsButton, { pointerType: 'mouse' })
    fireEvent.pointerEnter(projectsButton, { pointerType: 'mouse' })
    fireEvent.mouseOver(projectsButton)

    expect(await screen.findByRole('tooltip')).toHaveTextContent('Projects')
  })

  it('keeps the brand row draggable on macOS for top-left window moves', () => {
    platformState.isMac = true

    renderRail()

    const rail = screen.getByRole('navigation', { name: 'Global actions' })
    expect(rail.className).not.toContain('pt-[32px]')
    expect(rail.querySelector('[data-tauri-drag-region="true"]')).not.toBeNull()
  })

  it('opens the terminal board from the project section', () => {
    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Open the terminal board' }))

    expect(mockNavigate).toHaveBeenCalledWith('/terminals')
  })

  it('shows a live terminal count on the board action', () => {
    useTerminalStore.setState({
      terminals: [
        { id: 't-live', name: 'zsh', ptyId: 'pty-1', shell: 'zsh' },
        { id: 't-dead', name: 'old', shell: 'zsh' }
      ]
    })

    renderRail()

    expect(screen.getByRole('button', { name: 'Open the terminal board' })).toHaveTextContent('1')
  })

  it('enters the regular project workspace via the projects action', () => {
    render(
      <MemoryRouter initialEntries={['/conversations']}>
        <ActivityRail />
      </MemoryRouter>
    )

    fireEvent.click(screen.getByRole('button', { name: 'Open projects' }))

    expect(mockNavigate).toHaveBeenCalledWith('/')
  })

  it('opens git changes when a project is available', () => {
    const onOpenGitChanges = vi.fn()
    render(
      <MemoryRouter>
        <ActivityRail onOpenGitChanges={onOpenGitChanges} canOpenGitChanges />
      </MemoryRouter>
    )

    fireEvent.click(screen.getByRole('button', { name: 'Open git changes' }))

    expect(onOpenGitChanges).toHaveBeenCalledTimes(1)
  })

  it('disables git changes when no project is available', () => {
    const onOpenGitChanges = vi.fn()
    render(
      <MemoryRouter>
        <ActivityRail onOpenGitChanges={onOpenGitChanges} canOpenGitChanges={false} />
      </MemoryRouter>
    )

    const gitButton = screen.getByRole('button', { name: 'Open git changes' })
    expect(gitButton).toBeDisabled()
    fireEvent.click(gitButton)
    expect(onOpenGitChanges).not.toHaveBeenCalled()
  })

  it('opens the conversations area from the rail chat toggle', () => {
    render(
      <MemoryRouter>
        <ActivityRail />
      </MemoryRouter>
    )

    const chatButton = screen.getByRole('button', { name: 'Open the conversations area' })
    expect(chatButton).toHaveAttribute('aria-pressed', 'false')
    fireEvent.click(chatButton)
    expect(mockNavigate).toHaveBeenCalledWith('/conversations')
  })

  it('returns to the project workspace when the conversations area is active', () => {
    render(
      <MemoryRouter initialEntries={['/conversations']}>
        <ActivityRail />
      </MemoryRouter>
    )

    const chatButton = screen.getByRole('button', { name: 'Open the conversations area' })
    expect(chatButton).toHaveAttribute('aria-pressed', 'true')
    fireEvent.click(chatButton)
    expect(mockNavigate).toHaveBeenCalledWith('/')
  })

  it('toggles the SSH panel via persistence-aware updater on click', async () => {
    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Hide SSH panel' }))

    await waitFor(() => {
      expect(mockUpdatePanelVisibility).toHaveBeenCalledWith('sshPanelVisible', false)
    })
  })

  it('shows error toast when SSH panel persistence update fails', async () => {
    mockUpdatePanelVisibility.mockRejectedValueOnce(new Error('persist failed'))

    renderRail()

    fireEvent.click(screen.getByRole('button', { name: 'Hide SSH panel' }))

    await waitFor(() => {
      expect(mockToastError).toHaveBeenCalledWith('persist failed')
    })
  })

  it('disables the SSH rail button with a desktop-only reason on web', () => {
    const prev = tauriRef.current
    tauriRef.current = false
    try {
      renderRail()
      const sshButton = screen.getByRole('button', { name: /SSH/i })
      expect(sshButton).toBeDisabled()
      expect(sshButton.parentElement).toHaveAttribute('data-rail-tooltip', 'SSH is desktop-only')
    } finally {
      tauriRef.current = prev
    }
  })
})
