import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { ListEmptyState, ListLoadingState, ListRow, ListRowMeta, ListRowStatus } from './index'
import { pathBasename } from './path-basename'

describe('list primitives', () => {
  it('renders title, preview, and meta', () => {
    render(
      <ListRow
        title="Session title"
        preview="You: last turn"
        meta={<ListRowMeta items={['Claude', '12 msgs']} />}
      />
    )
    expect(screen.getByText('Session title')).toBeInTheDocument()
    expect(screen.getByText('You: last turn')).toBeInTheDocument()
    expect(screen.getByText('Claude')).toBeInTheDocument()
    expect(screen.getByText('12 msgs')).toBeInTheDocument()
  })

  it('keeps details collapsed until expanded', () => {
    const { rerender } = render(<ListRow title="Row" details={<span>cwd</span>} expanded={false} />)
    expect(screen.queryByText('cwd')).not.toBeInTheDocument()
    rerender(<ListRow title="Row" details={<span>cwd</span>} expanded />)
    expect(screen.getByText('cwd')).toBeInTheDocument()
  })

  it('marks the active row without a 28px chip ring', () => {
    render(<ListRow title="Active" active />)
    const row = screen.getByText('Active').closest('[data-list-row]')
    expect(row).toHaveAttribute('data-active')
    expect(row).toHaveClass('bg-sidebar-accent')
    expect(row).not.toHaveClass('min-h-7', 'ring-1')
  })

  it('renders empty and loading states as designed status regions', () => {
    const { rerender } = render(<ListEmptyState message="Nothing here" />)
    expect(screen.getByRole('status')).toHaveTextContent('Nothing here')
    rerender(<ListLoadingState label="Scanning" rows={2} />)
    expect(screen.getByRole('status')).toHaveAttribute('aria-busy', 'true')
  })

  it('pathBasename keeps the last folder', () => {
    expect(pathBasename('/Users/dev/projects/termul')).toBe('termul')
    expect(pathBasename('C:\\work\\repo')).toBe('repo')
    expect(pathBasename(null)).toBe('')
  })

  it('forwards row activation', () => {
    const onClick = vi.fn()
    render(<ListRow title="Open" onClick={onClick} />)
    screen.getByRole('button', { name: /Open/ }).click()
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it('colors the status chip: accent for need, connection lamp for working, muted idle', () => {
    const { rerender } = render(<ListRowStatus status="need" label="Need you" />)
    expect(screen.getByText('Need you')).toHaveAttribute('data-list-row-status', 'need')
    expect(screen.getByText('Need you')).toHaveClass('text-accent')

    rerender(<ListRowStatus status="working" label="Working" />)
    expect(screen.getByText('Working')).toHaveAttribute('data-list-row-status', 'working')
    expect(screen.getByText('Working')).toHaveClass('text-connection')

    rerender(<ListRowStatus status="idle" label="Idle" />)
    expect(screen.getByText('Idle')).toHaveAttribute('data-list-row-status', 'idle')
    expect(screen.getByText('Idle')).toHaveClass('text-muted-foreground')
    expect(screen.getByText('Idle')).not.toHaveClass('text-accent')
  })
})
