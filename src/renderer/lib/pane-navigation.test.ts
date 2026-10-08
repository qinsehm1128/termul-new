import { afterEach, describe, expect, it, vi } from 'vitest'
import { useWorkspaceStore } from '@/stores/workspace-store'
import { DEFAULT_KEYBOARD_SHORTCUTS } from '@/types/settings'
import type { LeafNode, PaneNode } from '@/types/workspace.types'
import { findAdjacentPane, findPaneInDirection, handlePaneShortcut } from './pane-navigation'

const leaf = (id: string): LeafNode => ({ type: 'leaf', id, tabs: [], activeTabId: null })

// ┌───┬───┐
// │   │ B │
// │ A ├───┤
// │   │ C │
// └───┴───┘
const layout: PaneNode = {
  type: 'split',
  id: 'root',
  direction: 'horizontal',
  sizes: [50, 50],
  children: [
    leaf('A'),
    {
      type: 'split',
      id: 'right',
      direction: 'vertical',
      sizes: [50, 50],
      children: [leaf('B'), leaf('C')]
    }
  ]
}

describe('findPaneInDirection', () => {
  it('moves across and within splits', () => {
    expect(findPaneInDirection(layout, 'A', 'right')).toBe('B')
    expect(findPaneInDirection(layout, 'B', 'down')).toBe('C')
    expect(findPaneInDirection(layout, 'C', 'up')).toBe('B')
    expect(findPaneInDirection(layout, 'C', 'left')).toBe('A')
  })

  it('returns null at the edge of the layout', () => {
    expect(findPaneInDirection(layout, 'A', 'left')).toBeNull()
    expect(findPaneInDirection(layout, 'B', 'up')).toBeNull()
    expect(findPaneInDirection(layout, 'A', 'down')).toBeNull()
  })

  it('prefers the pane that shares the most edge', () => {
    // Left column A (0–50%) over D; right column B (0–40%) over C. C overlaps
    // A by 10% of the height and D by 50%, so left from C is D, not A.
    const skewed: PaneNode = {
      type: 'split',
      id: 'root',
      direction: 'horizontal',
      sizes: [50, 50],
      children: [
        {
          type: 'split',
          id: 'left',
          direction: 'vertical',
          sizes: [50, 50],
          children: [leaf('A'), leaf('D')]
        },
        {
          type: 'split',
          id: 'right',
          direction: 'vertical',
          sizes: [40, 60],
          children: [leaf('B'), leaf('C')]
        }
      ]
    }
    expect(findPaneInDirection(skewed, 'C', 'left')).toBe('D')
    expect(findPaneInDirection(skewed, 'A', 'right')).toBe('B')
  })
})

describe('findAdjacentPane', () => {
  it('cycles through panes in layout order', () => {
    expect(findAdjacentPane(layout, 'A', 1)).toBe('B')
    expect(findAdjacentPane(layout, 'C', 1)).toBe('A')
    expect(findAdjacentPane(layout, 'A', -1)).toBe('C')
  })

  it('has nowhere to go with a single pane', () => {
    expect(findAdjacentPane(leaf('A'), 'A', 1)).toBeNull()
  })
})

describe('handlePaneShortcut', () => {
  const initial = useWorkspaceStore.getState()
  // Bind the pane actions to plain ctrl keys so the test is platform-neutral.
  const keys: Record<string, string> = {
    splitRight: 'ctrl+d',
    splitDown: 'ctrl+shift+d',
    focusPaneRight: 'ctrl+alt+arrowright',
    focusPaneNext: 'ctrl+]',
    togglePaneZoom: 'ctrl+shift+enter'
  }
  const getKey = (id: string) => keys[id] ?? ''
  const press = (init: KeyboardEventInit) =>
    new KeyboardEvent('keydown', { ctrlKey: true, ...init })

  afterEach(() => {
    useWorkspaceStore.setState({
      root: initial.root,
      activePaneId: initial.activePaneId,
      fullscreenPaneId: null
    })
    document.body.innerHTML = ''
  })

  it('splits the active pane right or down', () => {
    useWorkspaceStore.setState({ root: layout, activePaneId: 'B' })
    const split = vi.fn()
    expect(handlePaneShortcut(press({ key: 'd', code: 'KeyD' }), getKey, split)).toBe(true)
    expect(
      handlePaneShortcut(press({ key: 'D', code: 'KeyD', shiftKey: true }), getKey, split)
    ).toBe(true)
    expect(split.mock.calls).toEqual([
      ['B', 'right'],
      ['B', 'bottom']
    ])
  })

  it('moves the active pane and keyboard focus to the neighbour', () => {
    useWorkspaceStore.setState({ root: layout, activePaneId: 'A' })
    document.body.innerHTML =
      '<div data-pane-id="B"><div class="invisible"><textarea id="hidden"></textarea></div>' +
      '<textarea id="visible"></textarea></div>'
    const handled = handlePaneShortcut(
      press({ key: 'ArrowRight', code: 'ArrowRight', altKey: true }),
      getKey,
      vi.fn()
    )
    expect(handled).toBe(true)
    expect(useWorkspaceStore.getState().activePaneId).toBe('B')
    expect(document.activeElement?.id).toBe('visible')
  })

  it('cycles to the next pane', () => {
    useWorkspaceStore.setState({ root: layout, activePaneId: 'C' })
    handlePaneShortcut(press({ key: ']', code: 'BracketRight' }), getKey, vi.fn())
    expect(useWorkspaceStore.getState().activePaneId).toBe('A')
  })

  it('toggles the active pane fullscreen', () => {
    useWorkspaceStore.setState({ root: layout, activePaneId: 'B', fullscreenPaneId: null })
    handlePaneShortcut(press({ key: 'Enter', code: 'Enter', shiftKey: true }), getKey, vi.fn())
    expect(useWorkspaceStore.getState().fullscreenPaneId).toBe('B')
  })

  it('leaves other keys alone', () => {
    useWorkspaceStore.setState({ root: layout, activePaneId: 'B' })
    const split = vi.fn()
    expect(handlePaneShortcut(press({ key: 'k', code: 'KeyK' }), getKey, split)).toBe(false)
    expect(split).not.toHaveBeenCalled()
  })

  it('ships macOS defaults that are ⌘ only', () => {
    // ctrl+d is the shell's EOF on Windows/Linux and must stay with the shell.
    expect(DEFAULT_KEYBOARD_SHORTCUTS.splitRight.defaultKey).toBe('cmd+d')
    expect(DEFAULT_KEYBOARD_SHORTCUTS.splitDown.defaultKey).toBe('cmd+shift+d')
  })
})
