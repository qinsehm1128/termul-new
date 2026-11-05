import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { TocHeading } from '@/hooks/use-toc-headings'
import { TocPanel } from './TocPanel'

const settings = vi.hoisted(() => ({ maxHeadingLevel: 3 }))
vi.mock('@/stores/toc-settings-store', () => ({
  useTocSettingsStore: (selector: (state: unknown) => unknown) =>
    selector({ settings, setMaxHeadingLevel: vi.fn() })
}))

function createDocument() {
  let headings: TocHeading[] = [
    { id: 'one', blockId: 'block-1', text: 'First', level: 1 },
    { id: 'two', blockId: 'block-2', text: 'Second', level: 2 }
  ]
  let version = 0
  const listeners = new Set<() => void>()
  const api = {
    getHeadings: vi.fn(() => headings),
    scrollToBlock: vi.fn(),
    getDocumentVersion: () => version,
    subscribeDocumentChange: vi.fn((listener: () => void) => {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    })
  }
  return {
    api,
    listeners,
    update(next: TocHeading[]) {
      headings = next
      version += 1
      listeners.forEach((listener) => {
        listener()
      })
    }
  }
}

afterEach(() => {
  settings.maxHeadingLevel = 3
  vi.unstubAllGlobals()
})

describe('TocPanel BlockNote document updates', () => {
  it('keeps the active heading and observer on parent renders, but refreshes real document changes', () => {
    const doc = createDocument()
    const container = document.createElement('div')
    container.innerHTML = `
      <div data-node-type="blockContainer" data-id="block-1"><div data-content-type="heading"></div></div>
      <div data-node-type="blockContainer" data-id="block-2"><div data-content-type="heading"></div></div>
    `
    const disconnect = vi.fn()
    const construct = vi.fn()
    let observerCallback: IntersectionObserverCallback | undefined
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        constructor(callback: IntersectionObserverCallback) {
          construct()
          observerCallback = callback
        }
        observe = vi.fn()
        disconnect = disconnect
      }
    )
    const { rerender, unmount } = render(
      <TocPanel editorMode="blocknote" blocknote={{ ...doc.api }} container={container} />
    )
    act(() => {
      observerCallback?.(
        [
          {
            target: container.children[1],
            isIntersecting: true,
            boundingClientRect: { top: 20 }
          } as IntersectionObserverEntry
        ],
        {} as IntersectionObserver
      )
    })
    expect(screen.getByText('Second').closest('button')).toHaveClass('bg-sidebar-accent')
    const reads = doc.api.getHeadings.mock.calls.length
    rerender(<TocPanel editorMode="blocknote" blocknote={{ ...doc.api }} container={container} />)
    expect(doc.api.getHeadings).toHaveBeenCalledTimes(reads)
    expect(screen.getByText('Second').closest('button')).toHaveClass('bg-sidebar-accent')
    expect(construct).toHaveBeenCalledTimes(1)
    expect(disconnect).not.toHaveBeenCalled()

    // A document change publishes before the debounced markdown parent round-trip.
    act(() =>
      doc.update([
        { id: 'one', blockId: 'block-1', text: 'Renamed', level: 1 },
        { id: 'two', blockId: 'block-2', text: 'Second', level: 3 }
      ])
    )
    expect(screen.queryByText('First')).not.toBeInTheDocument()
    expect(screen.getByText('Renamed')).toBeInTheDocument()
    expect(screen.getByText('Second').closest('button')).toHaveClass('bg-sidebar-accent')
    expect(construct).toHaveBeenCalledTimes(1)
    settings.maxHeadingLevel = 1
    rerender(<TocPanel editorMode="blocknote" blocknote={{ ...doc.api }} container={container} />)
    expect(screen.queryByText('Second')).not.toBeInTheDocument()
    expect(screen.getByText('Renamed').closest('button')).toHaveClass('bg-sidebar-accent')
    unmount()
    expect(doc.listeners.size).toBe(0)
  })

  it('unsubscribes when switching to CodeMirror and tracks visible lines there', () => {
    const doc = createDocument()
    const scrollToLine = vi.fn()
    const content = '# Source\n\n## Section\n'
    const { rerender } = render(
      <TocPanel editorMode="blocknote" blocknote={doc.api} container={null} />
    )
    expect(doc.listeners.size).toBe(1)
    rerender(
      <TocPanel
        editorMode="codemirror"
        codemirror={{
          content,
          scrollToLine,
          visibleRange: { startLine: 1, endLine: 2 }
        }}
      />
    )
    expect(doc.listeners.size).toBe(0)
    expect(screen.queryByText('First')).not.toBeInTheDocument()
    expect(screen.getByText('Source').closest('button')).toHaveClass('bg-sidebar-accent')
    rerender(
      <TocPanel
        editorMode="codemirror"
        codemirror={{
          content,
          scrollToLine,
          visibleRange: { startLine: 3, endLine: 4 }
        }}
      />
    )
    expect(screen.getByText('Section').closest('button')).toHaveClass('bg-sidebar-accent')
    fireEvent.click(screen.getByText('Section'))
    expect(scrollToLine).toHaveBeenCalledWith(3)
  })

  it('falls back to the observed heading when the clicked heading is removed', () => {
    const doc = createDocument()
    render(<TocPanel editorMode="blocknote" blocknote={doc.api} container={null} />)
    fireEvent.click(screen.getByText('Second'))
    expect(screen.getByText('Second').closest('button')).toHaveClass('bg-sidebar-accent')
    act(() => doc.update([{ id: 'one', blockId: 'block-1', text: 'First', level: 1 }]))
    expect(screen.queryByText('Second')).not.toBeInTheDocument()
    expect(screen.getByText('First').closest('button')).toHaveClass('bg-sidebar-accent')
  })

  it('updates after asynchronous document replacement without a parent render', async () => {
    const doc = createDocument()
    const { unmount } = render(
      <TocPanel editorMode="blocknote" blocknote={doc.api} container={null} />
    )
    fireEvent.click(screen.getByText('Second'))
    expect(doc.api.scrollToBlock).toHaveBeenCalledWith('block-2')
    await act(async () => {
      await Promise.resolve()
      doc.update([{ id: 'new', blockId: 'new-block', text: 'New document', level: 1 }])
    })
    expect(screen.queryByText('First')).not.toBeInTheDocument()
    expect(screen.queryByText('Second')).not.toBeInTheDocument()
    expect(screen.getByText('New document').closest('button')).toHaveClass('bg-sidebar-accent')
    unmount()
    expect(doc.listeners.size).toBe(0)
  })
})
