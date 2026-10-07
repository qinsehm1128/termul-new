import { invoke } from '@tauri-apps/api/core'
import { isMac } from './platform'
import { isTauriContext } from './tauri-runtime'

/**
 * Finder-compatible file pasteboard: what the file tree copies can be pasted in
 * Finder or a chat app, and what Finder copies or drags in can be pasted or
 * dropped into the tree. macOS desktop only — elsewhere every call is inert and
 * the tree keeps its in-app clipboard.
 */
export function isFilePasteboardSupported(): boolean {
  return isMac && isTauriContext()
}

export interface PasteboardFiles {
  /** Bumps on every pasteboard write by any app. */
  changeCount: number
  paths: string[]
}

/** Put `paths` on the system pasteboard as files; resolves the new change count. */
export async function writePasteboardFilePaths(paths: string[]): Promise<number | null> {
  if (!isFilePasteboardSupported()) return null
  try {
    return await invoke<number>('pasteboard_write_file_paths', { paths })
  } catch (error) {
    console.error('Failed to write files to the pasteboard:', error)
    return null
  }
}

export async function readPasteboardFilePaths(): Promise<PasteboardFiles | null> {
  if (!isFilePasteboardSupported()) return null
  try {
    return await invoke<PasteboardFiles>('pasteboard_read_file_paths')
  } catch (error) {
    console.error('Failed to read files from the pasteboard:', error)
    return null
  }
}

/**
 * Paths of the drag being dropped. Tauri's native drag-drop is off (the tree
 * and composer use HTML5 drag events), so a Finder drop reaches the webview
 * without paths; the drag pasteboard still holds them.
 */
export async function readDragPasteboardFilePaths(): Promise<string[]> {
  if (!isFilePasteboardSupported()) return []
  try {
    return await invoke<string[]>('drag_pasteboard_file_paths')
  } catch (error) {
    console.error('Failed to read the dropped file paths:', error)
    return []
  }
}
