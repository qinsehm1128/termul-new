import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'
import { TerminalQuickSwitcher } from './TerminalQuickSwitcher'

// cmdk scrolls the highlighted item into view; jsdom has no layout.
window.HTMLElement.prototype.scrollIntoView = vi.fn()

const navigate = vi.fn()
vi.mock('react-router-dom', () => ({ useNavigate: () => navigate }))

const openBoardTerminal = vi.fn()
vi.mock('@/lib/terminal-board-navigation', () => ({
  openBoardTerminal: (options: unknown) => openBoardTerminal(options)
}))

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key })
}))

function terminal(id: string, name: string, projectId?: string): Terminal {
  return { id, name, projectId, shell: 'bash', ptyId: `pty-${id}` }
}

const onClose = vi.fn()

function renderSwitcher(): void {
  render(<TerminalQuickSwitcher isOpen onClose={onClose} />)
}

function optionLabels(): string[] {
  return screen.getAllByRole('option').map((node) => node.textContent ?? '')
}

describe('TerminalQuickSwitcher', () => {
  beforeEach(() => {
    navigate.mockClear()
    openBoardTerminal.mockClear()
    onClose.mockClear()
    useProjectStore.setState({
      projects: [
        { id: 'p1', name: 'Alpha', color: 'blue' },
        { id: 'p2', name: 'Beta', color: 'green' }
      ],
      groups: [],
      activeProjectId: 'p1',
      activeGroupId: null
    })
    useTerminalStore.setState({
      terminals: [
        terminal('t1', 'build', 'p1'),
        terminal('t2', 'server', 'p2'),
        terminal('t3', 'logs', 'p1')
      ],
      activeTerminalId: 't1',
      recentTerminalIds: []
    })
  })

  it('should list terminals from every project, not just the active one', () => {
    renderSwitcher()
    // Opens on the widest scope — reaching for search means looking past the
    // bounded row.
    expect(optionLabels().map((label) => label.split(/(?=[A-Z])/)[0])).toEqual([
      'build',
      'server',
      'logs'
    ])
  })

  it('should lead with the most recently visited terminal', () => {
    useTerminalStore.setState({ recentTerminalIds: ['t3', 't2'] })
    renderSwitcher()

    // Open-then-Enter has to be the same jump as the last-terminal shortcut.
    expect(optionLabels()[0]).toContain('logs')
    expect(optionLabels()[1]).toContain('server')
  })

  it('should jump through the cross-project open path and close', () => {
    renderSwitcher()

    fireEvent.click(screen.getByRole('option', { name: /server/ }))

    expect(openBoardTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: 'p2', terminalId: 't2' })
    )
    expect(useTerminalStore.getState().activeTerminalId).toBe('t2')
    expect(onClose).toHaveBeenCalled()
  })

  it('should narrow to the active project when the project scope is picked', () => {
    renderSwitcher()

    fireEvent.click(screen.getByText('switcher.scope.project'))

    expect(optionLabels().every((label) => !label.includes('server'))).toBe(true)
    expect(optionLabels()).toHaveLength(2)
  })

  it('should match on the project name as well as the terminal name', () => {
    renderSwitcher()

    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'Beta' } })

    // "which project was that in" has to be a usable query.
    expect(optionLabels()).toHaveLength(1)
    expect(optionLabels()[0]).toContain('server')
  })
})
