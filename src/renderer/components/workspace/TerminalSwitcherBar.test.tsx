import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'
import { TerminalSwitcherBar } from './TerminalSwitcherBar'

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

/** Chip label plus its terminal-count badge, e.g. `P1 2`. */
function projectChips(): string[] {
  return screen.getAllByRole('tab').map((node) => node.textContent ?? '')
}

function seed(activeGroupId: string | null): void {
  useProjectStore.setState({
    projects: [
      { id: 'p1', name: 'P1', color: 'blue' },
      { id: 'p2', name: 'P2', color: 'green' },
      { id: 'p3', name: 'P3', color: 'red' }
    ],
    groups: [{ id: 'g1', name: 'G1', projectIds: ['p1', 'p2'] }],
    activeProjectId: 'p1',
    activeGroupId
  })
  useTerminalStore.setState({
    terminals: [terminal('t1', 'p1'), terminal('t2', 'p2'), terminal('t3', 'p3')],
    activeTerminalId: 't1',
    recentTerminalIds: []
  })
}

describe('TerminalSwitcherBar', () => {
  beforeEach(() => {
    navigate.mockClear()
    openBoardTerminal.mockClear()
    seed(null)
  })

  it('should show one chip per project rather than one per terminal', () => {
    // The reported symptom: two terminals of one project rendered as two
    // visually identical neighbouring chips.
    useTerminalStore.setState({
      terminals: [terminal('t1', 'p1'), terminal('t1b', 'p1'), terminal('t2', 'p2')],
      activeTerminalId: 't1'
    })
    render(<TerminalSwitcherBar />)

    expect(projectChips()).toEqual(['P12', 'P21'])
  })

  it('should show the terminal count on each project chip', () => {
    useTerminalStore.setState({
      terminals: [terminal('t1', 'p1'), terminal('t1b', 'p1'), terminal('t1c', 'p1')],
      activeTerminalId: 't1'
    })
    render(<TerminalSwitcherBar />)

    // The badge is what explains why one chip stands for several terminals.
    expect(screen.getByRole('tab').textContent).toBe('P13')
  })

  it('should narrow to the group when the group scope is picked', () => {
    seed('g1')
    render(<TerminalSwitcherBar />)

    fireEvent.click(screen.getByText('switcher.scope.group'))

    // p3 lives outside the group and must stay out of the row.
    expect(projectChips()).toEqual(['P11', 'P21'])
  })

  it('should narrow to the group as soon as one is clicked in the sidebar', () => {
    const { rerender } = render(<TerminalSwitcherBar />)
    expect(projectChips()).toEqual(['P11', 'P21', 'P31'])

    // Selecting a group is itself the request to see its projects — the user
    // should not have to pick the scope again afterwards.
    useProjectStore.getState().selectGroup('g1')
    rerender(<TerminalSwitcherBar />)

    expect(projectChips()).toEqual(['P11', 'P21'])
  })

  it('should enable the group step from project membership, not group scope', () => {
    // The reported symptom: the step was permanently unlit. `selectProject`
    // deliberately exits group *scope*, so keying off activeGroupId alone made
    // the step dead for anyone who works by clicking projects.
    useProjectStore.setState({ activeGroupId: null, activeProjectId: 'p1' })
    render(<TerminalSwitcherBar />)

    // p1 belongs to g1, so the group step is meaningful even with no scope set.
    expect(screen.getByText('switcher.scope.group')).not.toBeDisabled()
  })

  it('should keep the group step disabled for a project in no group', () => {
    useProjectStore.setState({ activeGroupId: null, activeProjectId: 'p3' })
    render(<TerminalSwitcherBar />)

    // p3 is outside g1 — there is genuinely no group to widen to.
    expect(screen.getByText('switcher.scope.group')).toBeDisabled()
  })

  it('should offer only the group and all steps', () => {
    render(<TerminalSwitcherBar />)

    // `project` is gone on purpose: at project granularity it always resolves
    // to exactly one chip, so it is a step that cannot change what you see.
    // Whether `group` is usable is asserted separately above.
    expect(screen.queryByText('switcher.scope.project')).toBeNull()
    expect(screen.getByText('switcher.scope.group')).toBeTruthy()
    expect(screen.getByText('switcher.scope.all')).toBeTruthy()
  })

  it('should enable the group step once a group is selected', () => {
    seed('g1')
    render(<TerminalSwitcherBar />)

    expect(screen.getByText('switcher.scope.group')).not.toBeDisabled()
  })

  it('should route a click through the cross-project open path', () => {
    seed('g1')
    render(<TerminalSwitcherBar />)
    fireEvent.click(screen.getByText('switcher.scope.group'))

    fireEvent.click(screen.getByRole('tab', { name: /P2/ }))

    // Not a bare selectTerminal: p2 is another project, so the jump has to go
    // through the project switch as well.
    expect(openBoardTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: 'p2', terminalId: 't2' })
    )
    expect(useTerminalStore.getState().activeTerminalId).toBe('t2')
  })

  it('should open the most recently visited terminal of the clicked project', () => {
    useTerminalStore.setState({
      terminals: [terminal('t1', 'p1'), terminal('a', 'p2'), terminal('b', 'p2')],
      activeTerminalId: 't1',
      recentTerminalIds: ['b', 'a']
    })
    render(<TerminalSwitcherBar />)

    fireEvent.click(screen.getByRole('tab', { name: /P2/ }))

    // Not p2's first terminal in store order — returning to a project should
    // resume where the user left it.
    expect(openBoardTerminal).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: 'p2', terminalId: 'b' })
    )
  })

  it('should mark the chip owning the active terminal as selected', () => {
    seed(null)
    render(<TerminalSwitcherBar />)

    expect(screen.getByRole('tab', { name: /P1/ })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByRole('tab', { name: /P2/ })).toHaveAttribute('aria-selected', 'false')
  })

  it('should stay on screen with a single terminal', () => {
    // The reported symptom: one terminal in the active project hid the whole
    // bar, so the scope steps and the list toggle were unreachable.
    useTerminalStore.setState({ terminals: [terminal('t1', 'p1')], activeTerminalId: 't1' })
    render(<TerminalSwitcherBar />)

    expect(screen.getByText('switcher.scope.all')).toBeTruthy()
    expect(projectChips()).toEqual(['P11'])
  })

  it('should render nothing when there are no terminals at all', () => {
    useTerminalStore.setState({ terminals: [], activeTerminalId: '' })
    const { container } = render(<TerminalSwitcherBar />)
    expect(container.firstChild).toBeNull()
  })
  it('should toggle the vertical list from the bar', () => {
    const onToggleList = vi.fn()
    render(<TerminalSwitcherBar isListOpen={false} onToggleList={onToggleList} />)

    fireEvent.click(screen.getByRole('button', { name: 'switcher.showList' }))

    expect(onToggleList).toHaveBeenCalled()
  })

  it('should report the list state so the toggle reads as pressed', () => {
    render(<TerminalSwitcherBar isListOpen onToggleList={vi.fn()} />)

    expect(screen.getByRole('button', { name: 'switcher.hideList' })).toHaveAttribute(
      'aria-pressed',
      'true'
    )
  })

  it('should omit the toggle when no list is wired up', () => {
    render(<TerminalSwitcherBar />)

    expect(screen.queryByRole('button', { name: /switcher\.(show|hide)List/ })).toBeNull()
  })
})
