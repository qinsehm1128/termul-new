import { describe, expect, it } from 'vitest'
import type { LeafNode, PaneNode } from '@/types/workspace.types'
import { findAdjacentPane, findPaneInDirection } from './pane-navigation'

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
