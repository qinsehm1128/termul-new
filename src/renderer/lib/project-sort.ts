/**
 * Project list ordering.
 *
 * Two modes, because the list serves two habits. `'name'` reads like a file
 * manager and stays predictable as projects are added. `'manual'` preserves the
 * drag order, which encodes intent a name cannot (a working set at the top).
 * Sorting by name while drag was still live would have made a dragged project
 * snap back, so the mode is what decides whether drag is offered at all.
 */

import type { Project } from '@/types/project'

export type ProjectSortMode = 'name' | 'manual'

/**
 * Locale-aware, digit-aware collation — the comparison a file manager makes.
 *
 * - `numeric` so `项目9` precedes `项目10` instead of sorting as text.
 * - `sensitivity: 'base'` so case and accents do not split otherwise-adjacent
 *   names (`Beta` next to `beta`, not in a separate uppercase block).
 *
 * Built once: constructing an `Intl.Collator` per comparison is the classic way
 * to make a sort quietly expensive, and this runs on every sidebar render.
 */
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' })

/**
 * Orders projects for display.
 *
 * Returns the input array unchanged (same reference) in `'manual'` mode, so
 * callers memoising on identity do no extra work and the drag order passes
 * through untouched.
 */
export function sortProjects<T extends Pick<Project, 'id' | 'name'>>(
  projects: T[],
  mode: ProjectSortMode
): T[] {
  if (mode !== 'name') return projects

  return [...projects].sort((a, b) => {
    const byName = collator.compare(a.name, b.name)
    // Ids break ties so two projects sharing a name keep a stable order
    // instead of swapping places on unrelated re-renders.
    return byName !== 0 ? byName : a.id.localeCompare(b.id)
  })
}

/** Whether the user may reorder the list by dragging in this mode. */
export function isManualOrderingEnabled(mode: ProjectSortMode): boolean {
  return mode === 'manual'
}
