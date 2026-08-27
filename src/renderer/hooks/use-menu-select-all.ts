import { useEffect } from 'react'
import { listen, type UnlistenFn } from '@/lib/tauri-event'
import { cleanupTauriListener, isTauriContext } from '@/lib/tauri-runtime'
import { selectAllFocused } from '@/lib/text-edit-ops'

/**
 * Must stay equal to `MENU_EVENT_SELECT_ALL` in `src-tauri/src/lib.rs`.
 */
const MENU_EVENT_SELECT_ALL = 'menu:select-all'

/**
 * Handles the native Edit ▸ Select All item (and its Cmd/Ctrl+A accelerator).
 *
 * The menu item used to be the OS-predefined one, whose macOS key equivalent
 * AppKit consumes before the webview sees a keydown — so CodeMirror's `Mod-a`
 * binding was dead, and the `selectAll:` selector it fired selected only the
 * DOM. A virtualized editor keeps just the lines near the viewport in the DOM,
 * which is why "select all then copy" returned the rendered slice instead of
 * the file. Same class of problem as the Cmd+W / `menu:close-tab` listener.
 *
 * `selectAllFocused()` asks the focused editor for its whole document; the
 * `execCommand` fallback reproduces the document-wide selection the predefined
 * item gave when focus was not on an editable, so nothing regresses outside
 * the editor.
 */
export function useMenuSelectAll(): void {
  useEffect(() => {
    if (!isTauriContext()) return

    let unlisten: Promise<UnlistenFn> | undefined

    try {
      unlisten = listen(MENU_EVENT_SELECT_ALL, () => {
        if (selectAllFocused()) return
        document.execCommand('selectAll')
      })
    } catch (error) {
      console.error('[useMenuSelectAll] Failed to register select-all menu listener:', error)
      return
    }

    return () => {
      cleanupTauriListener(unlisten)
    }
  }, [])
}
