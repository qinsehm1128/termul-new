import { afterEach, describe, expect, it } from 'vitest'
import { useEditorStore } from '@/stores/editor-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'
import { claimFileOpens, releaseShownFile, showOpenedFile } from './file-open-target'

const file = '/repo/readme.md'

function editorTabPaths(): string[] {
  return getAllLeafPanes(useWorkspaceStore.getState().root).flatMap((leaf) =>
    leaf.tabs.flatMap((tab) => (tab.type === 'editor' ? [tab.filePath] : []))
  )
}

afterEach(() => {
  useEditorStore.setState({ openFiles: new Map() })
})

describe('file open target', () => {
  it('opens a pane tab unless a page has claimed file opens', () => {
    const shown: string[] = []
    const release = claimFileOpens((path) => shown.push(path))
    showOpenedFile(file)
    expect(shown).toEqual([file])
    expect(editorTabPaths()).not.toContain(file)

    release()
    showOpenedFile(file)
    expect(shown).toEqual([file])
    expect(editorTabPaths()).toContain(file)

    // Shown in a pane as well: letting go of the preview must not close it.
    useEditorStore.setState({
      openFiles: new Map([[file, { filePath: file, isDirty: false } as never]])
    })
    releaseShownFile(file)
    expect(useEditorStore.getState().openFiles.has(file)).toBe(true)
  })

  it('keeps a released file that has unsaved edits', () => {
    useEditorStore.setState({
      openFiles: new Map([
        ['/repo/draft.md', { filePath: '/repo/draft.md', isDirty: true } as never]
      ])
    })
    releaseShownFile('/repo/draft.md')
    expect(useEditorStore.getState().openFiles.has('/repo/draft.md')).toBe(true)
  })
})
