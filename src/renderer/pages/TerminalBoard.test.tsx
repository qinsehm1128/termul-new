import { fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import TerminalBoard from './TerminalBoard'

const { openBoardTerminal, openBoardProject } = vi.hoisted(() => ({
  openBoardTerminal: vi.fn(),
  openBoardProject: vi.fn()
}))

vi.mock('@/lib/terminal-board-navigation', () => ({
  openBoardTerminal,
  openBoardProject
}))

describe('TerminalBoard', () => {
  beforeEach(() => {
    openBoardTerminal.mockClear()
    openBoardProject.mockClear()
    useProjectStore.setState({
      projects: [{ id: 'p-cost', name: 'logistics-api', color: 'blue', path: '/srv/logistics' }],
      groups: [{ id: 'g-ns', name: 'Acme Corp', projectIds: ['p-cost'], isCollapsed: false }],
      activeProjectId: 'p-cost',
      isLoaded: true
    })
    useTerminalStore.setState({
      terminals: [
        {
          id: 'term-1',
          name: 'cost-shell',
          projectId: 'p-cost',
          shell: 'zsh',
          cwd: '/Users/dev/projects/logistics-api',
          ptyId: 'pty-1',
          healthStatus: 'running',
          viewState: 'visible'
        }
      ],
      activeTerminalId: 'term-1',
      ptyIdIndex: new Map(),
      cleanupRecoveries: {}
    })
  })

  it('shows group, project, and terminal and jumps on click', () => {
    render(
      <MemoryRouter>
        <TerminalBoard />
      </MemoryRouter>
    )

    expect(screen.getByRole('heading', { name: 'Terminal board' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Acme Corp' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'logistics-api' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /cost-shell/ })).toBeInTheDocument()
    expect(screen.getByText('Live')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'logistics-api' }))
    expect(openBoardProject).toHaveBeenCalledWith(expect.objectContaining({ projectId: 'p-cost' }))

    fireEvent.click(screen.getByRole('button', { name: /cost-shell/ }))
    expect(openBoardTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: 'p-cost', terminalId: 'term-1' })
    )
  })

  it('filters the board by group, project, or terminal name', () => {
    render(
      <MemoryRouter>
        <TerminalBoard />
      </MemoryRouter>
    )

    fireEvent.change(screen.getByLabelText('Search terminals'), { target: { value: 'missing' } })
    expect(screen.getByRole('status')).toHaveTextContent('No matching terminals')

    // 'Acme' appears only in the group name — not in the project ('logistics-api'),
    // the terminal ('cost-shell') or the path — so a hit here can only have come
    // from matching on the group.
    fireEvent.change(screen.getByLabelText('Search terminals'), { target: { value: 'Acme' } })
    expect(screen.getByRole('button', { name: /cost-shell/ })).toBeInTheDocument()
  })
})
