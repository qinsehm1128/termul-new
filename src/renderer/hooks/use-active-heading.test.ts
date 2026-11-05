import { act, renderHook } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  getActiveHeadingFromVisibleRange,
  useBlockNoteActiveHeading,
  useCodeMirrorActiveHeading
} from './use-active-heading'
import type { TocHeading } from './use-toc-headings'

const headings: TocHeading[] = [
  { id: 'heading-line-1', level: 1, text: 'Title', line: 1 },
  { id: 'heading-line-4', level: 2, text: 'Section', line: 4 },
  { id: 'heading-line-8', level: 3, text: 'Details', line: 8 }
]

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('use-active-heading', () => {
  it('returns the first heading when no visible range is available', () => {
    expect(getActiveHeadingFromVisibleRange(headings, undefined)).toBe('heading-line-1')
  })

  it('returns the nearest heading at or above the visible line', () => {
    expect(getActiveHeadingFromVisibleRange(headings, { startLine: 1, endLine: 5 })).toBe(
      'heading-line-1'
    )
    expect(getActiveHeadingFromVisibleRange(headings, { startLine: 6, endLine: 10 })).toBe(
      'heading-line-4'
    )
    expect(getActiveHeadingFromVisibleRange(headings, { startLine: 10, endLine: 12 })).toBe(
      'heading-line-8'
    )
  })

  it('returns undefined when there are no headings', () => {
    expect(getActiveHeadingFromVisibleRange([], { startLine: 1, endLine: 2 })).toBeUndefined()
  })

  it('computes active heading in the CodeMirror hook', () => {
    const { result, rerender } = renderHook(
      ({ visibleRange }) => useCodeMirrorActiveHeading({ headings, visibleRange }),
      {
        initialProps: {
          visibleRange: { startLine: 2, endLine: 5 }
        }
      }
    )

    expect(result.current).toBe('heading-line-1')

    rerender({ visibleRange: { startLine: 8, endLine: 12 } })

    expect(result.current).toBe('heading-line-8')
  })

  it('preserves the active heading and observer for semantically unchanged headings', () => {
    const container = document.createElement('div')
    container.innerHTML = `
      <div data-node-type="blockContainer" data-id="block-1"><div data-content-type="heading"></div></div>
      <div data-node-type="blockContainer" data-id="block-2"><div data-content-type="heading"></div></div>
    `
    const blockHeadings: TocHeading[] = [
      { id: 'toc-1', blockId: 'block-1', level: 1, text: 'Title' },
      { id: 'toc-2', blockId: 'block-2', level: 2, text: 'Section' }
    ]
    const observe = vi.fn()
    const disconnect = vi.fn()
    let observerCallback: IntersectionObserverCallback | undefined
    const construct = vi.fn()
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        constructor(callback: IntersectionObserverCallback) {
          construct()
          observerCallback = callback
        }
        observe = observe
        disconnect = disconnect
      }
    )
    const { result, rerender, unmount } = renderHook(
      ({ headings }) => useBlockNoteActiveHeading({ headings, container }),
      { initialProps: { headings: blockHeadings } }
    )
    act(() => {
      observerCallback?.(
        [
          {
            target: container.children[1],
            isIntersecting: true,
            boundingClientRect: { top: 24 }
          } as IntersectionObserverEntry
        ],
        {} as IntersectionObserver
      )
    })
    expect(result.current).toBe('toc-2')

    rerender({ headings: blockHeadings.map((heading) => ({ ...heading })) })
    expect(result.current).toBe('toc-2')
    expect(construct).toHaveBeenCalledTimes(1)
    expect(disconnect).not.toHaveBeenCalled()

    // Text/level edits update the TOC, but do not change the observed block identities.
    rerender({
      headings: blockHeadings.map((heading) => ({ ...heading, text: 'Edited', level: 3 }))
    })
    expect(result.current).toBe('toc-2')
    expect(construct).toHaveBeenCalledTimes(1)

    // A real heading addition must rebind, without resetting a still-valid selection.
    const added = document.createElement('div')
    added.innerHTML = '<div data-content-type="heading"></div>'
    added.dataset.nodeType = 'blockContainer'
    added.dataset.id = 'block-3'
    container.appendChild(added)
    rerender({
      headings: [...blockHeadings, { id: 'toc-3', blockId: 'block-3', level: 2, text: 'New' }]
    })
    expect(result.current).toBe('toc-2')
    expect(construct).toHaveBeenCalledTimes(2)
    expect(disconnect).toHaveBeenCalledTimes(1)
    expect(observe).toHaveBeenCalledWith(added)
    act(() => {
      observerCallback?.(
        [
          {
            target: added,
            isIntersecting: true,
            boundingClientRect: { top: 12 }
          } as IntersectionObserverEntry
        ],
        {} as IntersectionObserver
      )
    })
    expect(result.current).toBe('toc-3')

    // Removing the active heading falls back, and clearing headings clears selection.
    rerender({ headings: [blockHeadings[0]] })
    expect(result.current).toBe('toc-1')
    rerender({ headings: [] })
    expect(result.current).toBeUndefined()
    expect(disconnect).toHaveBeenCalledTimes(3)
    unmount()
  })

  it('rebinds on container and enabled changes, and cleans up on unmount', () => {
    const createContainer = () => {
      const container = document.createElement('div')
      container.innerHTML =
        '<div data-node-type="blockContainer" data-id="toc-1"><div data-content-type="heading"></div></div>'
      return container
    }
    const first = createContainer()
    const second = createContainer()
    const observe = vi.fn()
    const disconnect = vi.fn()
    const roots: Element[] = []
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        constructor(_callback: IntersectionObserverCallback, options: IntersectionObserverInit) {
          roots.push(options.root as Element)
        }
        observe = observe
        disconnect = disconnect
      }
    )
    const { rerender, unmount } = renderHook(
      ({ container, isEnabled }) =>
        useBlockNoteActiveHeading({
          headings: [{ id: 'toc-1', text: 'Title', level: 1 }],
          container,
          isEnabled
        }),
      { initialProps: { container: first, isEnabled: true } }
    )
    expect(roots).toEqual([first])
    expect(observe).toHaveBeenLastCalledWith(first.children[0])
    rerender({ container: second, isEnabled: true })
    expect(roots).toEqual([first, second])
    expect(disconnect).toHaveBeenCalledTimes(1)
    expect(observe).toHaveBeenLastCalledWith(second.children[0])
    rerender({ container: second, isEnabled: false })
    expect(disconnect).toHaveBeenCalledTimes(2)
    expect(roots).toHaveLength(2)
    rerender({ container: second, isEnabled: true })
    expect(roots).toHaveLength(3)
    unmount()
    expect(disconnect).toHaveBeenCalledTimes(3)
  })

  it('maps visible BlockNote block ids back to TOC ids', () => {
    const container = document.createElement('div')
    const headingElement = document.createElement('div')
    const headingContent = document.createElement('div')
    headingElement.setAttribute('data-node-type', 'blockContainer')
    headingElement.setAttribute('data-id', 'block-1')
    headingContent.setAttribute('data-content-type', 'heading')
    headingElement.appendChild(headingContent)
    container.appendChild(headingElement)

    const blockHeadings: TocHeading[] = [
      { id: 'toc-heading-1', blockId: 'block-1', level: 1, text: 'Title' }
    ]

    let observerCallback: IntersectionObserverCallback | undefined

    class MockIntersectionObserver {
      constructor(callback: IntersectionObserverCallback) {
        observerCallback = callback
      }

      observe = vi.fn()
      disconnect = vi.fn()
      unobserve = vi.fn()
      root = null
      rootMargin = '0px'
      thresholds = [0]
      takeRecords = () => []
    }

    vi.stubGlobal('IntersectionObserver', MockIntersectionObserver)

    const { result } = renderHook(() =>
      useBlockNoteActiveHeading({
        headings: blockHeadings,
        container,
        isEnabled: true
      })
    )

    act(() => {
      observerCallback?.(
        [
          {
            target: headingElement,
            isIntersecting: true,
            boundingClientRect: { top: 24 }
          } as unknown as IntersectionObserverEntry
        ],
        {} as IntersectionObserver
      )
    })

    expect(result.current).toBe('toc-heading-1')
  })
})
