import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useEditorStore } from '@/stores/editor-store'
import type { WorkspaceTab } from '@/stores/workspace-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import { PaneDndProvider, usePaneDnd } from './use-pane-dnd'

const mockFindPaneById = vi.fn()

vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: {
    getState: vi.fn()
  },
  editorTabId: (filePath: string) => `edit-${filePath}`,
  findPaneById: (...args: unknown[]) => mockFindPaneById(...args)
}))

vi.mock('@/stores/editor-store', () => ({
  useEditorStore: {
    getState: vi.fn()
  }
}))

function createDragEvent(payload?: unknown): React.DragEvent {
  const data = payload === undefined ? '' : JSON.stringify(payload)
  const dataTransfer = {
    setData: vi.fn(),
    getData: vi.fn().mockReturnValue(data),
    effectAllowed: 'move'
  } as unknown as DataTransfer

  return {
    dataTransfer,
    preventDefault: vi.fn(),
    stopPropagation: vi.fn()
  } as unknown as React.DragEvent
}

describe('use-pane-dnd routing', () => {
  const moveTabToPane = vi.fn()
  const moveTabToNewSplit = vi.fn()
  const addTabToPane = vi.fn()
  const splitPane = vi.fn()
  const reorderTabsInPane = vi.fn()
  const openFile = vi.fn()

  beforeEach(() => {
    moveTabToPane.mockReset()
    moveTabToNewSplit.mockReset()
    addTabToPane.mockReset()
    splitPane.mockReset()
    reorderTabsInPane.mockReset()
    mockFindPaneById.mockReset()
    openFile.mockReset()

    ;(useWorkspaceStore.getState as unknown as ReturnType<typeof vi.fn>).mockReturnValue({
      moveTabToPane,
      moveTabToNewSplit,
      addTabToPane,
      splitPane,
      reorderTabsInPane
    })

    ;(useEditorStore.getState as unknown as ReturnType<typeof vi.fn>).mockReturnValue({
      openFile
    })
  })

  it('routes center tab drop to moveTabToPane', () => {
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <PaneDndProvider>{children}</PaneDndProvider>
    )

    const { result } = renderHook(() => usePaneDnd(), { wrapper })

    const event = createDragEvent({
      type: 'tab',
      tabId: 'tab-1',
      sourcePaneId: 'pane-a'
    })

    act(() => {
      result.current.handleDrop('pane-b', 'center', event)
    })

    expect(moveTabToPane).toHaveBeenCalledWith('tab-1', 'pane-a', 'pane-b')
    expect(moveTabToNewSplit).not.toHaveBeenCalled()
  })

  it('routes edge tab drop to moveTabToNewSplit', () => {
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <PaneDndProvider>{children}</PaneDndProvider>
    )

    const { result } = renderHook(() => usePaneDnd(), { wrapper })

    const event = createDragEvent({
      type: 'tab',
      tabId: 'tab-1',
      sourcePaneId: 'pane-a'
    })

    act(() => {
      result.current.handleDrop('pane-b', 'left', event)
    })

    expect(moveTabToNewSplit).toHaveBeenCalledWith('tab-1', 'pane-a', 'pane-b', 'left')
    expect(moveTabToPane).not.toHaveBeenCalled()
  })

  it('routes center file drop to addTabToPane after openFile', async () => {
    openFile.mockResolvedValue(undefined)

    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <PaneDndProvider>{children}</PaneDndProvider>
    )

    const { result } = renderHook(() => usePaneDnd(), { wrapper })

    const event = createDragEvent({ type: 'file', filePath: '/project/src/app.ts' })

    act(() => {
      result.current.handleDrop('pane-b', 'center', event)
    })

    await Promise.resolve()
    await Promise.resolve()

    expect(openFile).toHaveBeenCalledWith('/project/src/app.ts')
    expect(addTabToPane).toHaveBeenCalledWith('pane-b', {
      type: 'editor',
      id: 'edit-/project/src/app.ts',
      filePath: '/project/src/app.ts'
    } satisfies WorkspaceTab)
    expect(splitPane).not.toHaveBeenCalled()
  })

  it('routes edge file drop to splitPane with position', async () => {
    openFile.mockResolvedValue(undefined)

    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <PaneDndProvider>{children}</PaneDndProvider>
    )

    const { result } = renderHook(() => usePaneDnd(), { wrapper })

    const event = createDragEvent({ type: 'file', filePath: '/project/src/app.ts' })

    act(() => {
      result.current.handleDrop('pane-b', 'top', event)
    })

    await Promise.resolve()
    await Promise.resolve()

    expect(splitPane).toHaveBeenCalledWith(
      'pane-b',
      'vertical',
      {
        type: 'editor',
        id: 'edit-/project/src/app.ts',
        filePath: '/project/src/app.ts'
      },
      'top'
    )
    expect(addTabToPane).not.toHaveBeenCalled()
  })

  it('ignores malformed payload without mutating store actions', () => {
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <PaneDndProvider>{children}</PaneDndProvider>
    )

    const { result } = renderHook(() => usePaneDnd(), { wrapper })

    const event = createDragEvent('not-json')
    ;(event.dataTransfer.getData as unknown as ReturnType<typeof vi.fn>).mockReturnValue('{invalid')

    act(() => {
      result.current.handleDrop('pane-b', 'center', event)
    })

    expect(moveTabToPane).not.toHaveBeenCalled()
    expect(moveTabToNewSplit).not.toHaveBeenCalled()
    expect(addTabToPane).not.toHaveBeenCalled()
    expect(splitPane).not.toHaveBeenCalled()
  })

  it('sets and clears shared preview target', () => {
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <PaneDndProvider>{children}</PaneDndProvider>
    )

    const { result } = renderHook(() => usePaneDnd(), { wrapper })

    act(() => {
      result.current.setPreviewTarget('pane-a', 'right')
    })

    expect(result.current.previewTarget).toEqual({ paneId: 'pane-a', position: 'right' })

    act(() => {
      result.current.clearPreviewTarget('pane-b')
    })

    expect(result.current.previewTarget).toEqual({ paneId: 'pane-a', position: 'right' })

    act(() => {
      result.current.clearPreviewTarget('pane-a', 'right')
    })

    expect(result.current.previewTarget).toBeNull()
  })

  describe('handleTabReorder', () => {
    /** Render the hook with a drag already in flight from `sourcePaneId`. */
    function dragging(tabId: string, sourcePaneId: string) {
      const wrapper = ({ children }: { children: React.ReactNode }) => (
        <PaneDndProvider>{children}</PaneDndProvider>
      )
      const { result } = renderHook(() => usePaneDnd(), { wrapper })
      act(() => {
        result.current.startTabDrag(tabId, sourcePaneId, createDragEvent())
      })
      return result
    }

    const twoTabs = {
      type: 'leaf',
      tabs: [
        { type: 'terminal', id: 'tab-1', terminalId: 'term-1' },
        { type: 'terminal', id: 'tab-2', terminalId: 'term-2' }
      ]
    }

    /**
     * Merging a torn-out tab back. The index is read straight off the target
     * pane because the dragged tab is not in that list yet — subtracting the
     * "source was earlier" offset here, as the same-pane path must, would land
     * it one slot to the left of where it was dropped.
     */
    it('moves a tab in from another pane at the aimed-at index', () => {
      mockFindPaneById.mockReturnValue(twoTabs)
      const result = dragging('tab-9', 'pane-b')

      act(() => {
        result.current.handleTabReorder('pane-a', 'tab-2', 'before')
      })
      expect(moveTabToPane).toHaveBeenCalledWith('tab-9', 'pane-b', 'pane-a', 1)

      act(() => {
        result.current.handleTabReorder('pane-a', 'tab-2', 'after')
      })
      expect(moveTabToPane).toHaveBeenLastCalledWith('tab-9', 'pane-b', 'pane-a', 2)

      // A cross-pane drop is a move, never a reorder of a list the tab is not in.
      expect(reorderTabsInPane).not.toHaveBeenCalled()
    })

    it('still reorders within one pane without moving anything', () => {
      mockFindPaneById.mockReturnValue(twoTabs)
      const result = dragging('tab-1', 'pane-a')

      act(() => {
        result.current.handleTabReorder('pane-a', 'tab-2', 'after')
      })

      expect(reorderTabsInPane).toHaveBeenCalledWith('pane-a', ['tab-2', 'tab-1'])
      expect(moveTabToPane).not.toHaveBeenCalled()
    })

    it('does nothing when a tab is dropped on itself', () => {
      mockFindPaneById.mockReturnValue(twoTabs)
      const result = dragging('tab-1', 'pane-a')

      act(() => {
        result.current.handleTabReorder('pane-a', 'tab-1', 'after')
      })

      expect(reorderTabsInPane).not.toHaveBeenCalled()
      expect(moveTabToPane).not.toHaveBeenCalled()
    })

    it('does nothing when the target tab is no longer in the target pane', () => {
      mockFindPaneById.mockReturnValue(twoTabs)
      const result = dragging('tab-9', 'pane-b')

      act(() => {
        result.current.handleTabReorder('pane-a', 'tab-gone', 'before')
      })

      expect(moveTabToPane).not.toHaveBeenCalled()
    })
  })
})
