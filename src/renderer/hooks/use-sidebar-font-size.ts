import { useEffect } from 'react'
import { useSidebarFontSize } from '@/stores/app-settings-store'

/** File tree rows stay this much larger than project list rows, as at the defaults (12 / 14px). */
const FILE_TREE_EXTRA_PX = 2

/** Set the row-size variables behind the `text-sidebar-row` / `text-tree-row` utilities. */
export function applySidebarFontSize(projectListPx: number): void {
  const root = document.documentElement.style
  root.setProperty('--sidebar-row-font-size', `${projectListPx}px`)
  root.setProperty('--tree-row-font-size', `${projectListPx + FILE_TREE_EXTRA_PX}px`)
}

/** Keep the project list / file tree text size in sync with the setting. Mount once at the app root. */
export function useAppliedSidebarFontSizeSync(): void {
  const size = useSidebarFontSize()
  useEffect(() => {
    applySidebarFontSize(size)
  }, [size])
}
