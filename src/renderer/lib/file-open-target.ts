import { toast } from 'sonner'
import { runtimeT } from '@/i18n/runtime'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { useEditorStore } from '@/stores/editor-store'
import { useFloatingDocStore } from '@/stores/floating-doc-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'

/** Where the click came from; decides which open-target setting applies. */
export type FileOpenSource = 'terminal' | 'file-explorer' | 'external'

/**
 * Where a file opened from the file tree is shown.
 *
 * The workspace panes are the default. A page that hides them (the quick
 * terminal page) claims file opens while it is mounted; otherwise the tab
 * lands in panes nobody can see and the click looks like it did nothing.
 */
type FileOpenTarget = (path: string) => void

let claimed: FileOpenTarget | null = null

/** Route file opens to `target` until the returned release is called. */
export function claimFileOpens(target: FileOpenTarget): () => void {
  claimed = target
  return () => {
    if (claimed === target) claimed = null
  }
}

/**
 * Show a file the editor store has already opened.
 *
 * Terminal and external opens honour their own settings (floating preview or
 * workspace tab); file-tree clicks keep landing in the panes because that is
 * the surface they came from.
 */
export function showOpenedFile(path: string, options: { source?: FileOpenSource } = {}): void {
  if (claimed) {
    claimed(path)
    return
  }

  const { settings } = useAppSettingsStore.getState()
  const target =
    options.source === 'terminal'
      ? settings.terminalFileOpenTarget
      : options.source === 'external'
        ? settings.externalFileOpenTarget
        : 'editor-tab'

  if (target === 'floating') {
    void useFloatingDocStore
      .getState()
      .openDocument(path)
      .catch(() => {
        toast.error(
          runtimeT('settings', 'floatingDoc.openFailed', 'Could not open {{path}}', { path })
        )
      })
    return
  }

  useWorkspaceStore.getState().addEditorTab(path)
}

/**
 * Let go of a file shown outside the panes. The editor store persists every
 * open file with the project, so one left behind would come back as a project
 * file. Kept when a pane tab shows it too, or when it has unsaved edits.
 */
export function releaseShownFile(path: string): void {
  const inPanes = getAllLeafPanes(useWorkspaceStore.getState().root).some((leaf) =>
    leaf.tabs.some((tab) => tab.type === 'editor' && tab.filePath === path)
  )
  const file = useEditorStore.getState().openFiles.get(path)
  if (inPanes || !file || file.isDirty) return
  useEditorStore.getState().closeFileIfIdle(path)
}
