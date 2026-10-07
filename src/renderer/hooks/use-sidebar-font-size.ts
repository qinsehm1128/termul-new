import { useEffect } from 'react'
import { useSidebarFontSize } from '@/stores/app-settings-store'
import { SIDEBAR_FONT_SIZE_OFFSETS, type SidebarFontSize } from '@/types/settings'

/** Default row sizes the `sidebarFontSize` steps are offsets from. */
const PROJECT_LIST_BASE_PX = 12
const FILE_TREE_BASE_PX = 14

/** Set the row-size variables behind the `text-sidebar-row` / `text-tree-row` utilities. */
export function applySidebarFontSize(size: SidebarFontSize): void {
  const offset = SIDEBAR_FONT_SIZE_OFFSETS[size]
  const root = document.documentElement.style
  root.setProperty('--sidebar-row-font-size', `${PROJECT_LIST_BASE_PX + offset}px`)
  root.setProperty('--tree-row-font-size', `${FILE_TREE_BASE_PX + offset}px`)
}

/** Keep the project list / file tree text size in sync with the setting. Mount once at the app root. */
export function useAppliedSidebarFontSizeSync(): void {
  const size = useSidebarFontSize()
  useEffect(() => {
    applySidebarFontSize(size)
  }, [size])
}
