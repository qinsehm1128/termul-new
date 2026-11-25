import { fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'
import { RightInspectorRail, resolveInspectorSegment } from './RightInspectorRail'

describe('resolveInspectorSegment', () => {
  it('focuses a newly activated tenant when available', () => {
    expect(resolveInspectorSegment('files', ['files', 'ssh'], 'ssh')).toBe('ssh')
    expect(resolveInspectorSegment('files', ['files', 'cli'], 'cli')).toBe('cli')
  })

  it('keeps the current segment when still available and nothing new opened', () => {
    expect(resolveInspectorSegment('cli', ['files', 'cli', 'ssh'], null)).toBe('cli')
  })

  it('falls back to the first available tenant when the active one closes', () => {
    expect(resolveInspectorSegment('ssh', ['files', 'cli'], null)).toBe('files')
    expect(resolveInspectorSegment('files', ['ssh'], null)).toBe('ssh')
  })
})

describe('RightInspectorRail', () => {
  const baseProps = {
    storageKey: 'test-right-rail',
    resizeTitle: 'Resize right rail',
    resizeLabel: 'Resize right rail',
    filesContent: <div data-testid="files-content">Files View</div>,
    cliContent: <div data-testid="cli-content">CLI View</div>,
    sshContent: <div data-testid="ssh-content">SSH View</div>
  }

  it('renders a single tenant directly without redundant segmented switcher chrome', () => {
    render(<RightInspectorRail {...baseProps} hasFiles hasCli={false} hasSsh={false} />)

    expect(screen.getByTestId('explorer-rail-fade')).toBeInTheDocument()
    expect(screen.getByTestId('file-explorer-panel-fade')).toHaveClass('opacity-100')
    expect(screen.getByTestId('files-content')).toBeInTheDocument()
    expect(screen.queryByRole('tablist', { name: 'Inspector panels' })).not.toBeInTheDocument()
  })

  it('renders segmented switcher when multiple tenants are active and switches cleanly', () => {
    render(<RightInspectorRail {...baseProps} hasFiles hasCli hasSsh />)

    const tablist = screen.getByRole('tablist', { name: 'Inspector panels' })
    expect(tablist).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Files' })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByRole('tab', { name: 'CLI' })).toHaveAttribute('aria-selected', 'false')
    expect(screen.getByRole('tab', { name: 'SSH' })).toHaveAttribute('aria-selected', 'false')

    fireEvent.click(screen.getByRole('tab', { name: 'SSH' }))
    expect(screen.getByRole('tab', { name: 'SSH' })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByTestId('ssh-content')).toBeInTheDocument()

    fireEvent.click(screen.getByRole('tab', { name: 'CLI' }))
    expect(screen.getByRole('tab', { name: 'CLI' })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByTestId('cli-content')).toBeInTheDocument()
  })

  it('auto-focuses SSH when it activates alongside Files', () => {
    const { rerender } = render(
      <RightInspectorRail {...baseProps} hasFiles hasCli={false} hasSsh={false} />
    )
    expect(screen.getByTestId('files-content')).toBeInTheDocument()

    rerender(<RightInspectorRail {...baseProps} hasFiles hasCli={false} hasSsh />)
    expect(screen.getByRole('tab', { name: 'SSH' })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByTestId('ssh-content')).toBeInTheDocument()
  })

  it('preserves local panel state across segment switches without unmounting', () => {
    function StatefulCli(): React.JSX.Element {
      const [count, setCount] = useState(0)
      return (
        <button type="button" onClick={() => setCount((c) => c + 1)}>
          cli-count:{count}
        </button>
      )
    }

    render(
      <RightInspectorRail
        {...baseProps}
        hasFiles
        hasCli
        hasSsh={false}
        cliContent={<StatefulCli />}
      />
    )

    fireEvent.click(screen.getByRole('tab', { name: 'CLI' }))
    fireEvent.click(screen.getByRole('button', { name: 'cli-count:0' }))
    expect(screen.getByRole('button', { name: 'cli-count:1' })).toBeInTheDocument()

    // Switch away to Files and back to CLI — count must remain 1 (no remount/rescan).
    fireEvent.click(screen.getByRole('tab', { name: 'Files' }))
    expect(screen.getByTestId('cli-session-panel-fade')).toHaveClass('hidden')
    fireEvent.click(screen.getByRole('tab', { name: 'CLI' }))
    expect(screen.getByTestId('cli-session-panel-fade')).not.toHaveClass('hidden')
    expect(screen.getByRole('button', { name: 'cli-count:1' })).toBeInTheDocument()
  })

  it('recovers cleanly after closing all tenants and reopening one', () => {
    const { rerender } = render(
      <RightInspectorRail {...baseProps} hasFiles hasCli={false} hasSsh={false} />
    )
    rerender(<RightInspectorRail {...baseProps} hasFiles={false} hasCli={false} hasSsh={false} />)
    rerender(<RightInspectorRail {...baseProps} hasFiles={false} hasCli hasSsh={false} />)

    expect(screen.getByTestId('cli-session-panel-fade')).not.toHaveClass('hidden')
    expect(screen.getByTestId('cli-content')).toBeInTheDocument()
  })
})
