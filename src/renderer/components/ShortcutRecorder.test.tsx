import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { DEFAULT_KEYBOARD_SHORTCUTS, type KeyboardShortcutsConfig } from '@/types/settings'
import { ShortcutRecorder } from './ShortcutRecorder'

vi.mock('@/lib/shortcut-capture', () => ({
  beginShortcutCapture: vi.fn(),
  endShortcutCapture: vi.fn()
}))

function config(overrides: Record<string, string> = {}): KeyboardShortcutsConfig {
  const result: KeyboardShortcutsConfig = {}
  for (const [id, shortcut] of Object.entries(DEFAULT_KEYBOARD_SHORTCUTS)) {
    result[id] = { ...shortcut, defaultKey: overrides[id] ?? shortcut.defaultKey }
  }
  return result
}

describe('ShortcutRecorder', () => {
  it('labels an unbound shortcut instead of showing an empty key', () => {
    const shortcuts = config({ clearTerminal: '' })
    render(
      <ShortcutRecorder
        shortcut={shortcuts.clearTerminal}
        allShortcuts={shortcuts}
        onUpdate={vi.fn()}
        onReset={vi.fn()}
      />
    )
    expect(screen.getByRole('button', { name: /Clear Terminal/ })).toHaveTextContent('Not set')
  })

  it('shows the conflicts of the saved key', () => {
    const shortcuts = config()
    render(
      <ShortcutRecorder
        shortcut={shortcuts.splitRight}
        allShortcuts={shortcuts}
        onUpdate={vi.fn()}
        onReset={vi.fn()}
        conflicts={[
          { kind: 'duplicate', otherId: 'newProject' },
          { kind: 'reserved', reservedId: 'projectSwitch' }
        ]}
      />
    )
    expect(screen.getByText('Conflicts with "New Project".')).toBeInTheDocument()
    expect(
      screen.getByText('Same key as project switching (⌘1–9), which Se keeps fixed')
    ).toBeInTheDocument()
  })

  it('holds a recorded reserved key until the user confirms it', () => {
    const shortcuts = config()
    const onUpdate = vi.fn()
    render(
      <ShortcutRecorder
        shortcut={shortcuts.splitRight}
        allShortcuts={shortcuts}
        onUpdate={onUpdate}
        onReset={vi.fn()}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: /Split Right/ }))
    fireEvent.keyDown(window, { key: '3', code: 'Digit3', ctrlKey: true })

    expect(onUpdate).not.toHaveBeenCalled()
    expect(
      screen.getByText(/Same key as project switching \(⌘1–9\), which Se keeps fixed/)
    ).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Use anyway' }))
    expect(onUpdate).toHaveBeenCalledWith('splitRight', 'ctrl+3')
  })
})
