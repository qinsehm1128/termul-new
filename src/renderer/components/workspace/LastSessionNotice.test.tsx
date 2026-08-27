import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useLastSessionStore } from '@/stores/last-session-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { LastSessionNotice } from './LastSessionNotice'

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, vars?: Record<string, unknown>) =>
      vars ? `${key}:${JSON.stringify(vars)}` : key
  })
}))

function seedSnapshot(): void {
  useLastSessionStore.setState({
    snapshot: {
      capturedAt: '2026-08-26T01:00:00.000Z',
      projects: [
        { projectId: 'p1', name: 'Alpha', terminalCount: 2, terminalNames: ['build', 'claude'] },
        { projectId: 'p2', name: 'Beta', terminalCount: 1, terminalNames: ['shell'] }
      ]
    },
    dismissed: false
  })
}

function liveTerminalsFor(projectIds: string[]): void {
  useTerminalStore.setState({
    terminals: projectIds.map((projectId, index) => ({
      id: `t${index}`,
      name: `t${index}`,
      shell: 'bash',
      projectId
    })) as never,
    activeTerminalId: 't0'
  })
}

function rowState(projectId: string): string | null {
  return (
    document
      .querySelector(`[data-session-project="${projectId}"]`)
      ?.getAttribute('data-session-restored') ?? null
  )
}

describe('LastSessionNotice', () => {
  beforeEach(() => {
    useLastSessionStore.getState().reset()
    useTerminalStore.setState({ terminals: [], activeTerminalId: '' })
  })

  it('names each project that had terminals and how many', () => {
    seedSnapshot()
    render(<LastSessionNotice onRestore={vi.fn()} />)

    expect(screen.getByText('Alpha')).toBeInTheDocument()
    expect(screen.getByText('Beta')).toBeInTheDocument()
    expect(screen.getByText(/"terminals":2/)).toBeInTheDocument()
    expect(screen.getByText(/"terminals":1/)).toBeInTheDocument()
  })

  it('lists the terminal names so a project is recognisable', () => {
    seedSnapshot()
    render(<LastSessionNotice onRestore={vi.fn()} />)

    expect(screen.getByText(/build, claude/)).toBeInTheDocument()
  })

  it('restores the project whose row was clicked', () => {
    seedSnapshot()
    const onRestore = vi.fn()
    render(<LastSessionNotice onRestore={onRestore} />)

    const row = document.querySelector('[data-session-project="p2"]')
    fireEvent.click(row?.querySelector('button') as Element)

    expect(onRestore).toHaveBeenCalledWith('p2')
  })

  it('stays open while projects are still missing', () => {
    // The whole point: the list is what the user is working through, so the
    // first restore must not delete the remaining answers.
    seedSnapshot()
    const { container, rerender } = render(<LastSessionNotice onRestore={vi.fn()} />)

    liveTerminalsFor(['p1'])
    rerender(<LastSessionNotice onRestore={vi.fn()} />)

    expect(container.firstChild).not.toBeNull()
    expect(screen.getByText('Beta')).toBeInTheDocument()
  })

  it('keeps a restored project on the list rather than dropping the row', () => {
    // Removing the row would shift the rows below out from under the cursor.
    seedSnapshot()
    const { rerender } = render(<LastSessionNotice onRestore={vi.fn()} />)

    liveTerminalsFor(['p1'])
    rerender(<LastSessionNotice onRestore={vi.fn()} />)

    expect(rowState('p1')).toBe('true')
    expect(rowState('p2')).toBe('false')
  })

  it('offers no restore button for a project that is already live', () => {
    seedSnapshot()
    liveTerminalsFor(['p1'])
    render(<LastSessionNotice onRestore={vi.fn()} />)

    expect(document.querySelector('[data-session-project="p1"] button')).toBeNull()
    expect(document.querySelector('[data-session-project="p2"] button')).not.toBeNull()
  })

  it('closes itself once every project is back', () => {
    seedSnapshot()
    liveTerminalsFor(['p1', 'p2'])

    const { container } = render(<LastSessionNotice onRestore={vi.fn()} />)

    expect(container.firstChild).toBeNull()
  })

  it('renders nothing when the last session had no terminals', () => {
    useLastSessionStore.setState({ snapshot: { capturedAt: null, projects: [] } })

    const { container } = render(<LastSessionNotice onRestore={vi.fn()} />)

    expect(container.firstChild).toBeNull()
  })

  it('stays gone after it is dismissed', () => {
    seedSnapshot()
    const { container, rerender } = render(<LastSessionNotice onRestore={vi.fn()} />)

    fireEvent.click(screen.getByRole('button', { name: 'lastSession.dismiss' }))
    rerender(<LastSessionNotice onRestore={vi.fn()} />)

    expect(container.firstChild).toBeNull()
  })
})
