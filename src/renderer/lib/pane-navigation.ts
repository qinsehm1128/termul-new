import { matchesShortcut } from '@/stores/keyboard-shortcuts-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'
import type { PaneNode } from '@/types/workspace.types'

export type PaneFocusDirection = 'left' | 'right' | 'up' | 'down'

interface Rect {
  x: number
  y: number
  w: number
  h: number
}

/** Each leaf pane's rectangle in a unit square, from the split sizes. */
function leafRects(node: PaneNode, rect: Rect, out: Map<string, Rect>): void {
  if (node.type === 'leaf') {
    out.set(node.id, rect)
    return
  }
  const total = node.sizes.reduce((sum, size) => sum + size, 0) || node.children.length
  let offset = 0
  node.children.forEach((child, index) => {
    const share = (node.sizes[index] ?? total / node.children.length) / total
    const childRect =
      node.direction === 'horizontal'
        ? { x: rect.x + offset * rect.w, y: rect.y, w: share * rect.w, h: rect.h }
        : { x: rect.x, y: rect.y + offset * rect.h, w: rect.w, h: share * rect.h }
    offset += share
    leafRects(child, childRect, out)
  })
}

const EPSILON = 1e-6

function overlap(a0: number, a1: number, b0: number, b1: number): number {
  return Math.min(a1, b1) - Math.max(a0, b0)
}

/**
 * The pane next to `fromPaneId` in `direction`: the nearest pane on that side
 * that shares an edge span with it, preferring the one it overlaps most.
 */
export function findPaneInDirection(
  root: PaneNode,
  fromPaneId: string,
  direction: PaneFocusDirection
): string | null {
  const rects = new Map<string, Rect>()
  leafRects(root, { x: 0, y: 0, w: 1, h: 1 }, rects)
  const from = rects.get(fromPaneId)
  if (!from) return null

  let best: { id: string; distance: number; shared: number } | null = null
  for (const [id, rect] of rects) {
    if (id === fromPaneId) continue
    let distance: number
    let shared: number
    switch (direction) {
      case 'left':
        distance = from.x - (rect.x + rect.w)
        shared = overlap(from.y, from.y + from.h, rect.y, rect.y + rect.h)
        break
      case 'right':
        distance = rect.x - (from.x + from.w)
        shared = overlap(from.y, from.y + from.h, rect.y, rect.y + rect.h)
        break
      case 'up':
        distance = from.y - (rect.y + rect.h)
        shared = overlap(from.x, from.x + from.w, rect.x, rect.x + rect.w)
        break
      case 'down':
        distance = rect.y - (from.y + from.h)
        shared = overlap(from.x, from.x + from.w, rect.x, rect.x + rect.w)
        break
    }
    if (distance < -EPSILON || shared <= EPSILON) continue
    if (
      !best ||
      distance < best.distance - EPSILON ||
      (Math.abs(distance - best.distance) <= EPSILON && shared > best.shared)
    ) {
      best = { id, distance, shared }
    }
  }
  return best?.id ?? null
}

/** The pane after (or before) `fromPaneId` in layout order, wrapping around. */
export function findAdjacentPane(root: PaneNode, fromPaneId: string, step: 1 | -1): string | null {
  const leaves = getAllLeafPanes(root)
  const index = leaves.findIndex((leaf) => leaf.id === fromPaneId)
  if (index === -1 || leaves.length < 2) return null
  return leaves[(index + step + leaves.length) % leaves.length].id
}

/**
 * Move keyboard focus into a pane's visible tab. Activating a pane in the
 * store does not move DOM focus, so without this the next keystroke would
 * still reach the terminal the user just left.
 */
export function focusPaneSurface(paneId: string): void {
  const pane = document.querySelector(`[data-pane-id="${CSS.escape(paneId)}"]`)
  if (!pane) return
  const target = Array.from(
    pane.querySelectorAll<HTMLElement>('textarea, [contenteditable="true"]')
  ).find((element) => !element.closest('.invisible'))
  target?.focus()
}

// Pane focus shortcuts and where each one moves focus.
const PANE_FOCUS_SHORTCUTS: readonly {
  id: string
  direction: PaneFocusDirection | 'next' | 'prev'
}[] = [
  { id: 'focusPaneLeft', direction: 'left' },
  { id: 'focusPaneRight', direction: 'right' },
  { id: 'focusPaneUp', direction: 'up' },
  { id: 'focusPaneDown', direction: 'down' },
  { id: 'focusPaneNext', direction: 'next' },
  { id: 'focusPanePrev', direction: 'prev' }
]

/**
 * Run the pane shortcut `event` names, if any: split the active pane, move
 * focus to a neighbour, or maximize. Returns whether the event was a pane
 * shortcut, so the caller can claim it.
 */
export function handlePaneShortcut(
  event: KeyboardEvent,
  getActiveKey: (id: string) => string,
  splitPane: (paneId: string, position: 'right' | 'bottom') => void
): boolean {
  const workspace = useWorkspaceStore.getState()
  const paneId = workspace.activePaneId

  const splitPosition = matchesShortcut(event, getActiveKey('splitRight'))
    ? 'right'
    : matchesShortcut(event, getActiveKey('splitDown'))
      ? 'bottom'
      : null
  if (splitPosition) {
    if (paneId) splitPane(paneId, splitPosition)
    return true
  }

  const focus = PANE_FOCUS_SHORTCUTS.find(({ id }) => matchesShortcut(event, getActiveKey(id)))
  if (focus) {
    const targetPaneId = !paneId
      ? null
      : focus.direction === 'next' || focus.direction === 'prev'
        ? findAdjacentPane(workspace.root, paneId, focus.direction === 'next' ? 1 : -1)
        : findPaneInDirection(workspace.root, paneId, focus.direction)
    if (targetPaneId) {
      workspace.setActivePane(targetPaneId)
      focusPaneSurface(targetPaneId)
    }
    return true
  }

  if (matchesShortcut(event, getActiveKey('togglePaneZoom'))) {
    if (paneId) workspace.togglePaneFullscreen(paneId)
    return true
  }
  return false
}
