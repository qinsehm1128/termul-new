import type { DirectoryEntry } from '@shared/types/filesystem.types'
import { act, fireEvent, render, screen } from '@testing-library/react'
import type { SVGProps } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ContextMenuContent } from '@/components/ui/context-menu'
import { LONG_PRESS_MS } from '@/hooks/use-tree-long-press-drag'
import { useFileExplorerStore } from '@/stores/file-explorer-store'
import { FileTreeNode } from './FileTreeNode'

// The store's own suite covers moveEntries; here the question is only whether
// the row decides to call it.
const moveEntries = vi.fn()

vi.mock('@/hooks/use-pane-dnd', () => ({
  usePaneDnd: () => ({
    startFileDrag: vi.fn()
  })
}))

vi.mock('./file-icon-map', () => ({
  getFileIcon: () => (props: SVGProps<SVGSVGElement>) => <svg data-testid="file-icon" {...props} />
}))

// Stub the Radix context-menu primitives with the stateful F2 pattern: the
// trigger opens on `contextmenu` (only if the child's onContextMenu did not
// call preventDefault — mirrors Radix's checkForDefaultPrevented), content
// renders only while open, Escape closes.
vi.mock('@/components/ui/context-menu', async () => {
  const React = await import('react')
  const MenuCtx = React.createContext<{ open: boolean; setOpen: (o: boolean) => void }>({
    open: false,
    setOpen: () => {}
  })
  return {
    ContextMenu: ({ children }: { children: React.ReactNode }) => {
      const [open, setOpen] = React.useState(false)
      return <MenuCtx.Provider value={{ open, setOpen }}>{children}</MenuCtx.Provider>
    },
    ContextMenuTrigger: ({
      children,
      asChild
    }: {
      children: React.ReactNode
      asChild?: boolean
    }) => {
      const { setOpen } = React.useContext(MenuCtx)
      const merged = (e: React.MouseEvent) => {
        if (e.defaultPrevented) return
        e.preventDefault()
        setOpen(true)
      }
      if (asChild && React.isValidElement(children)) {
        const child = children as React.ReactElement<{
          onContextMenu?: (e: React.MouseEvent) => void
        }>
        return React.cloneElement(child, {
          onContextMenu: (e: React.MouseEvent) => {
            child.props.onContextMenu?.(e)
            merged(e)
          }
        })
      }
      return <div onContextMenu={merged}>{children}</div>
    },
    ContextMenuContent: ({ children }: { children: React.ReactNode }) => {
      const { open } = React.useContext(MenuCtx)
      if (!open) return null
      return <div>{children}</div>
    }
  }
})

describe('FileTreeNode', () => {
  it('keeps long names on the truncate path without forcing the row wider', () => {
    const longName =
      'xxxxxxxxxxxxxxxxxxxxxxxxxxxx_xxxxxxxxxxxxxxxxxxxxxxxx_xxxxxxxxxxxxxxxx_rev.docx'

    render(
      <FileTreeNode
        entry={{
          path: `/project/${longName}`,
          name: longName,
          type: 'file',
          extension: 'docx',
          size: 1024,
          modifiedAt: Date.UTC(2026, 5, 10)
        }}
        depth={0}
        isExpanded={false}
        isSelected={false}
        isLoading={false}
        onToggle={vi.fn()}
        onSelect={vi.fn()}
        onContextMenu={vi.fn()}
      />
    )

    const nameEl = screen.getByText(longName)
    expect(nameEl).toHaveClass('min-w-0', 'flex-1', 'truncate')
    expect(nameEl.parentElement).toHaveClass('min-w-0', 'overflow-hidden')
  })

  it('marks the selected row with sidebar-accent and a lichen inset ring', () => {
    render(
      <FileTreeNode
        entry={{
          path: '/project/selected.ts',
          name: 'selected.ts',
          type: 'file',
          extension: 'ts',
          size: 32,
          modifiedAt: Date.UTC(2026, 5, 10)
        }}
        depth={0}
        isExpanded={false}
        isSelected
        isLoading={false}
        onToggle={vi.fn()}
        onSelect={vi.fn()}
        onContextMenu={vi.fn()}
      />
    )

    const selected = document.querySelector('[data-path="/project/selected.ts"]')
    expect(selected).toHaveClass(
      'h-7',
      'bg-sidebar-accent',
      'ring-1',
      'ring-inset',
      'ring-primary/35',
      'duration-150'
    )
    expect(selected).not.toHaveClass('bg-accent')
    expect(selected).not.toHaveClass('hover:bg-secondary/50')
  })

  it('uses sidebar-accent hover on idle rows', () => {
    render(
      <FileTreeNode
        entry={{
          path: '/project/idle.ts',
          name: 'idle.ts',
          type: 'file',
          extension: 'ts',
          size: 16,
          modifiedAt: Date.UTC(2026, 5, 10)
        }}
        depth={0}
        isExpanded={false}
        isSelected={false}
        isLoading={false}
        onToggle={vi.fn()}
        onSelect={vi.fn()}
        onContextMenu={vi.fn()}
      />
    )

    expect(document.querySelector('[data-path="/project/idle.ts"]')).toHaveClass(
      'hover:bg-sidebar-accent/50'
    )
  })

  it('exposes the entry path via data-path for header-action reveal (GH-540)', () => {
    render(
      <FileTreeNode
        entry={{
          path: '/project/src/deep',
          name: 'deep',
          type: 'directory',
          extension: null,
          size: 0,
          modifiedAt: Date.UTC(2026, 5, 10)
        }}
        depth={1}
        isExpanded={false}
        isSelected={false}
        isLoading={false}
        onToggle={vi.fn()}
        onSelect={vi.fn()}
        onContextMenu={vi.fn()}
      />
    )

    const row = document.querySelector('[data-path="/project/src/deep"]')
    expect(row).not.toBeNull()
    expect(row).toHaveTextContent('deep')
  })

  it('wraps the row in a Radix ContextMenu trigger that opens on right-click (F3 + F1/F2 guard)', () => {
    const onContextMenu = vi.fn()
    render(
      <FileTreeNode
        entry={{
          path: '/project/file.txt',
          name: 'file.txt',
          type: 'file',
          extension: '.txt',
          size: 100,
          modifiedAt: 0
        }}
        depth={0}
        isExpanded={false}
        isSelected={false}
        isLoading={false}
        onToggle={vi.fn()}
        onSelect={vi.fn()}
        onContextMenu={onContextMenu}
        renderContextMenu={() => (
          <ContextMenuContent>
            <span data-testid="node-menu-content">Rename file.txt</span>
          </ContextMenuContent>
        )}
      />
    )

    // Content is gated behind the open state — not visible before right-click.
    expect(screen.queryByTestId('node-menu-content')).not.toBeInTheDocument()

    // Right-click fires the child's onContextMenu (selection seeding), then the
    // F2 stub opens the menu (defaultPrevented is false because F1 removed
    // preventDefault from the real handlers — a re-introduction would skip open).
    fireEvent.contextMenu(screen.getByText('file.txt'))
    expect(onContextMenu).toHaveBeenCalledTimes(1)

    expect(screen.getByTestId('node-menu-content')).toBeInTheDocument()
    expect(screen.getByText('Rename file.txt')).toBeInTheDocument()
  })

  it('wires renderContextMenu to supply the declarative menu content (F3)', () => {
    const renderContextMenu = vi.fn(() => (
      <ContextMenuContent>
        <span data-testid="wired-content">Delete</span>
      </ContextMenuContent>
    ))
    render(
      <FileTreeNode
        entry={{
          path: '/project/src',
          name: 'src',
          type: 'directory',
          extension: null,
          size: 0,
          modifiedAt: 0
        }}
        depth={0}
        isExpanded={false}
        isSelected={false}
        isLoading={false}
        onToggle={vi.fn()}
        onSelect={vi.fn()}
        onContextMenu={vi.fn()}
        renderContextMenu={renderContextMenu}
      />
    )

    // renderContextMenu is invoked with the entry to build the declarative content.
    expect(renderContextMenu).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'src', path: '/project/src', type: 'directory' })
    )

    // Content appears only after right-click opens the menu.
    expect(screen.queryByTestId('wired-content')).not.toBeInTheDocument()
    fireEvent.contextMenu(screen.getByText('src'))
    expect(screen.getByTestId('wired-content')).toBeInTheDocument()
  })

  describe('drag to move', () => {
    const dir = (path: string, name: string): DirectoryEntry => ({
      path,
      name,
      type: 'directory',
      extension: null,
      size: 0,
      modifiedAt: 0
    })
    const file = (path: string, name: string): DirectoryEntry => ({
      path,
      name,
      type: 'file',
      extension: 'ts',
      size: 1,
      modifiedAt: 0
    })

    function renderNode(entry: DirectoryEntry): void {
      render(
        <FileTreeNode
          entry={entry}
          depth={0}
          isExpanded={false}
          isSelected={false}
          isLoading={false}
          onToggle={vi.fn()}
          onSelect={vi.fn()}
          onContextMenu={vi.fn()}
        />
      )
    }

    function row(path: string): HTMLElement {
      const node = document.querySelector(`[data-path="${path}"]`)
      if (!node) throw new Error(`no row for ${path}`)
      return node as HTMLElement
    }

    function dataTransfer(): DataTransfer {
      const store = new Map<string, string>()
      return {
        setData: (type: string, value: string) => store.set(type, value),
        getData: (type: string) => store.get(type) ?? '',
        effectAllowed: '',
        dropEffect: '',
        types: [] as string[]
      } as unknown as DataTransfer
    }

    beforeEach(() => {
      useFileExplorerStore.setState({
        dragPaths: [],
        selectedPaths: new Set<string>(),
        moveEntries
      })
      moveEntries.mockClear()
    })

    it('should let a directory be dragged, not just a file', () => {
      renderNode(dir('/project/src', 'src'))
      // Directories used to be undraggable, which made "move into that folder"
      // impossible for the folders themselves.
      expect(row('/project/src')).toHaveAttribute('draggable', 'true')
    })

    it('should drag the whole selection when the dragged row is part of it', () => {
      useFileExplorerStore.setState({
        selectedPaths: new Set(['/project/a.ts', '/project/b.ts'])
      })
      renderNode(file('/project/a.ts', 'a.ts'))

      fireEvent.dragStart(row('/project/a.ts'), { dataTransfer: dataTransfer() })

      expect(useFileExplorerStore.getState().dragPaths).toEqual(['/project/a.ts', '/project/b.ts'])
    })

    it('should drag only the row when it is outside the selection', () => {
      useFileExplorerStore.setState({ selectedPaths: new Set(['/project/other.ts']) })
      renderNode(file('/project/a.ts', 'a.ts'))

      fireEvent.dragStart(row('/project/a.ts'), { dataTransfer: dataTransfer() })

      expect(useFileExplorerStore.getState().dragPaths).toEqual(['/project/a.ts'])
    })

    it('should move the dragged entries into a directory that is dropped on', () => {
      useFileExplorerStore.setState({ dragPaths: ['/project/a.ts'] })
      renderNode(dir('/project/src', 'src'))

      fireEvent.drop(row('/project/src'), { dataTransfer: dataTransfer() })

      expect(moveEntries).toHaveBeenCalledWith(['/project/a.ts'], '/project/src')
    })

    it('should refuse a drop that would move a directory into its own subtree', () => {
      useFileExplorerStore.setState({ dragPaths: ['/project/src'] })
      renderNode(dir('/project/src/components', 'components'))

      fireEvent.drop(row('/project/src/components'), { dataTransfer: dataTransfer() })

      expect(moveEntries).not.toHaveBeenCalled()
    })

    it('should not treat a file row as a drop target', () => {
      useFileExplorerStore.setState({ dragPaths: ['/project/a.ts'] })
      renderNode(file('/project/b.ts', 'b.ts'))

      fireEvent.drop(row('/project/b.ts'), { dataTransfer: dataTransfer() })

      expect(moveEntries).not.toHaveBeenCalled()
    })

    it('should highlight a legal drop target while dragging over it', () => {
      useFileExplorerStore.setState({ dragPaths: ['/project/a.ts'] })
      renderNode(dir('/project/src', 'src'))

      fireEvent.dragOver(row('/project/src'), { dataTransfer: dataTransfer() })

      expect(row('/project/src')).toHaveClass('ring-primary')
    })

    it('should not highlight a directory that would reject the drop', () => {
      useFileExplorerStore.setState({ dragPaths: ['/project/src'] })
      renderNode(dir('/project/src/components', 'components'))

      fireEvent.dragOver(row('/project/src/components'), { dataTransfer: dataTransfer() })

      expect(row('/project/src/components')).not.toHaveClass('ring-primary')
    })

    it('should expose the entry type so touch hit-testing can reject file rows', () => {
      renderNode(dir('/project/src', 'src'))
      // resolveLongPressDropTarget reads this attribute; without it every drop
      // would land on whatever row the finger happened to be over.
      expect(row('/project/src')).toHaveAttribute('data-entry-type', 'directory')
    })

    it('should move entries when a long-press drag is released on a folder', () => {
      const target = document.createElement('div')
      target.setAttribute('data-path', '/project/lib')
      target.setAttribute('data-entry-type', 'directory')
      document.body.appendChild(target)
      document.elementFromPoint = vi.fn(() => target) as unknown as typeof document.elementFromPoint

      useFileExplorerStore.setState({ selectedPaths: new Set<string>() })
      renderNode(file('/project/a.ts', 'a.ts'))

      vi.useFakeTimers()
      try {
        fireEvent.pointerDown(row('/project/a.ts'), {
          pointerType: 'touch',
          clientX: 0,
          clientY: 0
        })
        act(() => {
          vi.advanceTimersByTime(LONG_PRESS_MS)
        })
        fireEvent.pointerUp(row('/project/a.ts'), { pointerType: 'touch', clientX: 1, clientY: 1 })
      } finally {
        vi.useRealTimers()
      }

      expect(moveEntries).toHaveBeenCalledWith(['/project/a.ts'], '/project/lib')
    })
  })
})
