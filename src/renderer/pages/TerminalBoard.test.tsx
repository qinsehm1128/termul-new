import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { _resetTerminalOutputClockForTests, noteTerminalOutput } from '@/lib/terminal-output-clock'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'
import TerminalBoard from './TerminalBoard'

const { openBoardTerminal, openBoardProject, readTerminalPreview } = vi.hoisted(() => ({
  openBoardTerminal: vi.fn(),
  openBoardProject: vi.fn(),
  readTerminalPreview: vi.fn()
}))

vi.mock('@/lib/terminal-board-navigation', () => ({
  openBoardTerminal,
  openBoardProject
}))

vi.mock('@/components/terminal/terminal-preview', () => ({ readTerminalPreview }))

function running(id: string, extras: Partial<Terminal> = {}): Terminal {
  return {
    id,
    name: id,
    projectId: 'p-cost',
    shell: 'zsh',
    cwd: '/Users/dev/projects/logistics-api',
    ptyId: `pty-${id}`,
    healthStatus: 'running',
    viewState: 'visible',
    ...extras
  }
}

function renderBoard(): void {
  render(
    <MemoryRouter>
      <TerminalBoard />
    </MemoryRouter>
  )
}

function rowNames(): string[] {
  return screen.getAllByRole('option').map((row) => row.getAttribute('id') ?? '')
}

/** Room for the preview: clicks select instead of opening. */
function makeWide(): void {
  vi.stubGlobal(
    'matchMedia',
    vi.fn(() => ({
      matches: true,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn()
    }))
  )
}

describe('TerminalBoard', () => {
  beforeEach(() => {
    openBoardTerminal.mockClear()
    openBoardProject.mockClear()
    readTerminalPreview.mockReset()
    readTerminalPreview.mockResolvedValue(['$ rspec', 'Allow? (y/n)'])
    _resetTerminalOutputClockForTests()
    useProjectStore.setState({
      projects: [
        { id: 'p-cost', name: 'logistics-api', color: 'blue', path: '/srv/logistics' },
        { id: 'p-loose', name: 'loose', color: 'green', path: '/srv/loose' }
      ],
      groups: [{ id: 'g-ns', name: 'Acme Corp', projectIds: ['p-cost'], isCollapsed: false }],
      activeProjectId: 'p-cost',
      isLoaded: true
    })
    useTerminalStore.setState({
      terminals: [
        running('cost-shell'),
        running('cost-server', { oscTitle: 'rails s' }),
        running('cost-done', { healthStatus: 'exited' }),
        running('loose-agent', {
          projectId: 'p-loose',
          kind: 'agent',
          agentName: 'Claude Code',
          agentState: 'blocked'
        })
      ],
      activeTerminalId: 'cost-shell',
      ptyIdIndex: new Map(),
      cleanupRecoveries: {}
    })
    noteTerminalOutput('pty-cost-server')
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('lists one row per terminal under its group and project, most urgent first', () => {
    renderBoard()

    expect(screen.getByRole('heading', { name: 'Terminal board' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Acme Corp' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Ungrouped' })).toBeInTheDocument()
    expect(rowNames()).toEqual([
      'terminal-board-row-cost-server',
      'terminal-board-row-cost-shell',
      'terminal-board-row-cost-done',
      'terminal-board-row-loose-agent'
    ])
    const server = screen.getByRole('option', { name: /cost-server/ })
    expect(within(server).getByText('Busy')).toBeInTheDocument()
    expect(within(server).getByText('rails s')).toBeInTheDocument()
    expect(
      within(screen.getByRole('option', { name: /cost-shell/ })).getByText('Idle')
    ).toBeInTheDocument()
    expect(
      within(screen.getByRole('option', { name: /cost-done/ })).getByText('Ended')
    ).toBeInTheDocument()
    const agent = screen.getByRole('option', { name: /loose-agent/ })
    expect(within(agent).getByText('Needs you')).toBeInTheDocument()
    expect(within(agent).getByText('Claude Code')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'logistics-api' }))
    expect(openBoardProject).toHaveBeenCalledWith(expect.objectContaining({ projectId: 'p-cost' }))
  })

  it('opens a terminal on click when there is no room for a preview', () => {
    renderBoard()

    fireEvent.click(screen.getByRole('option', { name: /cost-shell/ }))

    expect(openBoardTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: 'p-cost', terminalId: 'cost-shell' })
    )
  })

  it('filters by status, counting what each filter would show', () => {
    renderBoard()

    expect(screen.getByRole('radio', { name: 'Needs you 1' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('radio', { name: 'Ended 1' }))

    expect(rowNames()).toEqual(['terminal-board-row-cost-done'])
  })

  it('groups by status with each row naming its project', () => {
    renderBoard()

    fireEvent.click(screen.getByRole('radio', { name: 'Status' }))

    const needsYou = screen.getByRole('region', { name: 'Needs you' })
    expect(within(needsYou).getByText('loose')).toBeInTheDocument()
    expect(screen.getByRole('region', { name: 'Busy' })).toBeInTheDocument()
    expect(rowNames()[0]).toBe('terminal-board-row-loose-agent')
  })

  it('moves the selection with the arrow keys and opens it with Enter', () => {
    renderBoard()
    const list = screen.getByRole('listbox')

    fireEvent.keyDown(list, { key: 'ArrowDown' })
    fireEvent.keyDown(list, { key: 'ArrowDown' })
    expect(list).toHaveAttribute('aria-activedescendant', 'terminal-board-row-cost-shell')
    fireEvent.keyDown(list, { key: 'Enter' })

    expect(openBoardTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ terminalId: 'cost-shell' })
    )
  })

  it('previews the selected terminal and opens only on double-click when wide', async () => {
    makeWide()
    renderBoard()

    expect(
      await screen.findByRole('complementary', { name: 'Preview of cost-server' })
    ).toBeInTheDocument()
    expect(await screen.findByTestId('terminal-board-preview-screen')).toHaveTextContent(
      'Allow? (y/n)'
    )

    fireEvent.click(screen.getByRole('option', { name: /loose-agent/ }))
    expect(
      await screen.findByRole('complementary', { name: 'Preview of loose-agent' })
    ).toBeInTheDocument()
    expect(openBoardTerminal).not.toHaveBeenCalled()

    fireEvent.doubleClick(screen.getByRole('option', { name: /loose-agent/ }))
    expect(openBoardTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ terminalId: 'loose-agent' })
    )
  })

  it('ends a process only after confirmation', async () => {
    const terminate = vi.fn(async () => true)
    useTerminalStore.setState({ terminateTerminalResource: terminate })
    renderBoard()

    fireEvent.click(
      within(screen.getByRole('option', { name: /cost-shell/ })).getByRole('button', {
        name: 'End process'
      })
    )
    expect(terminate).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Terminate process' }))

    await waitFor(() => expect(terminate).toHaveBeenCalledWith('cost-shell'))
    expect(openBoardTerminal).not.toHaveBeenCalled()
  })

  it('leaves restarting and ending quick terminals to their own page', () => {
    useTerminalStore.setState((state) => ({
      terminals: [
        ...state.terminals,
        running('scratch', { projectId: undefined, quickTerminalId: 'qt-1' })
      ]
    }))
    renderBoard()

    const quick = screen.getByRole('option', { name: /scratch/ })
    expect(within(quick).getByRole('button', { name: 'Open' })).toBeInTheDocument()
    expect(within(quick).queryByRole('button', { name: 'Restart' })).toBeNull()
    expect(within(quick).queryByRole('button', { name: 'End process' })).toBeNull()
  })

  it('filters the board by group, project, or terminal name', () => {
    renderBoard()

    fireEvent.change(screen.getByLabelText('Search terminals'), { target: { value: 'missing' } })
    expect(screen.getByRole('status')).toHaveTextContent('No matching terminals')

    // 'Acme' appears only in the group name — not in a project, a terminal or a
    // path — so a hit here can only have come from matching on the group.
    fireEvent.change(screen.getByLabelText('Search terminals'), { target: { value: 'Acme' } })
    expect(rowNames()).toEqual([
      'terminal-board-row-cost-server',
      'terminal-board-row-cost-shell',
      'terminal-board-row-cost-done'
    ])
  })
})
