import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'
import { TerminalListPanel } from './TerminalListPanel'

const navigate = vi.fn()
vi.mock('react-router-dom', () => ({ useNavigate: () => navigate }))

const openBoardTerminal = vi.fn()
vi.mock('@/lib/terminal-board-navigation', () => ({
  openBoardTerminal: (options: unknown) => openBoardTerminal(options)
}))

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key })
}))

function terminal(id: string, projectId?: string): Terminal {
  return { id, name: id, projectId, shell: 'bash', ptyId: `pty-${id}` }
}

describe('TerminalListPanel', () => {
  beforeEach(() => {
    navigate.mockClear()
    openBoardTerminal.mockClear()
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
      terminals: [terminal('t1', 'p1'), terminal('t2', 'p2')],
      activeTerminalId: 't1',
      recentTerminalIds: []
    })
  })

  it('should list terminals from every project, unlike the bounded row', () => {
    render(<TerminalListPanel />)

    expect(screen.getByRole('button', { name: /t1/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: /t2/ })).toBeTruthy()
  })

  it('should group entries under their project name', () => {
    render(<TerminalListPanel />)

    expect(screen.getByText('Alpha')).toBeTruthy()
    expect(screen.getByText('Beta')).toBeTruthy()
  })

  it('should mark the active terminal', () => {
    render(<TerminalListPanel />)

    expect(screen.getByRole('button', { name: /t1/ })).toHaveAttribute('aria-current', 'true')
    expect(screen.getByRole('button', { name: /t2/ })).not.toHaveAttribute('aria-current')
  })

  it('should jump through the cross-project open path', () => {
    render(<TerminalListPanel />)

    fireEvent.click(screen.getByRole('button', { name: /t2/ }))

    expect(openBoardTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: 'p2', terminalId: 't2' })
    )
    expect(useTerminalStore.getState().activeTerminalId).toBe('t2')
  })
})
