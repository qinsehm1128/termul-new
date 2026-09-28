import type { RailItemId, RailLayoutEntry } from '@shared/types/navigation.types'
import { railItemMobility } from '@shared/types/navigation.types'

export const RAIL_DRAG_START_DISTANCE_PX = 6

export interface RailRowBox {
  kind: 'item' | 'divider' | 'utility'
  id: string
  entryIndex: number
  top: number
  height: number
}

export interface RailPointerDrop {
  /** Index in the current entry list to insert before. `entries.length` means after the last entry. */
  beforeEntryIndex: number
  pin: boolean
}

export function railDragExceeded(startY: number, clientY: number): boolean {
  return Math.abs(clientY - startY) >= RAIL_DRAG_START_DISTANCE_PX
}

/**
 * Pointer Y against row midpoints. Utility rows are a pin target, not a slot
 * in the sortable entry list. Dividers are slots so an item can move across
 * them, but they are never the drag subject.
 */
export function railPointerDrop(
  clientY: number,
  rows: readonly RailRowBox[],
  movingId: RailItemId
): RailPointerDrop {
  const sortable = rows.filter((row) => row.kind !== 'utility' && row.entryIndex >= 0)
  let beforeEntryIndex = 0
  if (sortable.length > 0) {
    beforeEntryIndex = sortable[sortable.length - 1].entryIndex + 1
    for (const row of sortable) {
      const midpoint = row.top + row.height / 2
      if (clientY < midpoint) {
        beforeEntryIndex = row.entryIndex
        break
      }
      beforeEntryIndex = row.entryIndex + 1
    }
  }

  const utility = rows.find((row) => row.kind === 'utility')
  const pin =
    railItemMobility(movingId) === 'utility' &&
    utility !== undefined &&
    clientY >= utility.top + utility.height / 2

  return { beforeEntryIndex, pin }
}

/** Convert a "insert before this current-list index" target into a post-removal index. */
export function railRestIndex(
  entries: readonly RailLayoutEntry[],
  movingId: RailItemId,
  beforeEntryIndex: number
): number {
  const movingIndex = entries.findIndex((entry) => entry.kind === 'item' && entry.id === movingId)
  if (movingIndex < 0) return beforeEntryIndex
  if (beforeEntryIndex > movingIndex) return beforeEntryIndex - 1
  return beforeEntryIndex
}
