import type { EditorView } from '@codemirror/view'
import { act, render } from '@testing-library/react'
import type { PropsWithChildren } from 'react'
import type { PanelOnResize } from 'react-resizable-panels'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useCodeMirror } from '@/hooks/use-codemirror'
import {
  EDITOR_REVEAL_LINE_EVENT,
  PENDING_REVEAL_LINE_GLOBAL,
  type PendingRevealLineWindow
} from '@/lib/editor-events'
import { CodeEditor } from './CodeEditor'

const mockFocus = vi.fn()
const mockSetContent = vi.fn()
const mockScrollToLine = vi.fn()
const mockRestoreViewState = vi.fn()

const { mobileRef, tocSettingsRef } = vi.hoisted(() => ({
  // Mutable so the mobile TOC-hide test can flip the shell on/off.
  mobileRef: { current: false as boolean },
  // Mutable so the TOC-visible path can be exercised (default isVisible=false
  // keeps the existing tests unchanged).
  tocSettingsRef: {
    current: {
      isLoaded: true,
      loadFailed: false,
      settings: { isVisible: false, width: 280 },
      setWidth: vi.fn()
    }
  }
}))

const panelRef = vi.hoisted(() => ({
  onResize: undefined as PanelOnResize | undefined,
  setLayout: vi.fn()
}))

vi.mock('@/components/ui/resizable', async () => {
  const { forwardRef, useImperativeHandle } = await import('react')
  return {
    ResizablePanelGroup: forwardRef<unknown, PropsWithChildren>(({ children }, ref) => {
      useImperativeHandle(ref, () => ({
        getLayout: () => [50, 50],
        setLayout: panelRef.setLayout
      }))
      return <div>{children}</div>
    }),
    ResizablePanel: ({ children, onResize }: PropsWithChildren<{ onResize?: PanelOnResize }>) => {
      if (onResize) panelRef.onResize = onResize
      return <div>{children}</div>
    },
    ResizableHandle: () => <div />
  }
})

vi.mock('@/hooks/use-codemirror', () => ({
  useCodeMirror: vi.fn()
}))

vi.mock('@/hooks/use-mobile-web-shell', () => ({
  useMobileWebShell: () => mobileRef.current,
  MOBILE_WEB_SHELL_MAX_PX: 767
}))

vi.mock('./TocPanel', () => ({
  TocPanel: () => <div data-toc-panel="toc" />
}))

vi.mock('@/stores/toc-settings-store', () => ({
  useTocSettingsStore: (selector: (state: unknown) => unknown) => selector(tocSettingsRef.current)
}))

const defaultProps = {
  filePath: '/project/src/example.ts',
  content: 'line 1\nline 2\nline 3',
  language: 'typescript',
  isVisible: true,
  onChange: vi.fn(),
  onCursorChange: vi.fn(),
  onScrollChange: vi.fn()
} as const

// Partial EditorView stub sufficient for CodeEditor's usage (only calls .focus()).
const fakeView = { focus: mockFocus } as unknown as EditorView

describe('CodeEditor', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    delete (global as PendingRevealLineWindow)[PENDING_REVEAL_LINE_GLOBAL]
    mobileRef.current = false
    tocSettingsRef.current.settings.isVisible = false

    vi.mocked(useCodeMirror).mockReturnValue({
      view: fakeView,
      isReady: true,
      setContent: mockSetContent,
      flushPendingContent: vi.fn(),
      scrollToLine: mockScrollToLine,
      restoreViewState: mockRestoreViewState,
      getVisibleLineRange: vi.fn(() => null)
    })
  })

  it('restores initial cursor/scroll state once without re-triggering on later cursor updates', () => {
    const { rerender } = render(
      <CodeEditor
        {...defaultProps}
        initialCursorPosition={{ line: 10, col: 3 }}
        initialScrollTop={240}
      />
    )

    expect(mockRestoreViewState).toHaveBeenCalledTimes(1)
    expect(mockRestoreViewState).toHaveBeenCalledWith(10, 3, 240)
    expect(mockScrollToLine).not.toHaveBeenCalled()

    rerender(
      <CodeEditor
        {...defaultProps}
        initialCursorPosition={{ line: 11, col: 1 }}
        initialScrollTop={240}
      />
    )

    expect(mockRestoreViewState).toHaveBeenCalledTimes(1)
    expect(mockScrollToLine).not.toHaveBeenCalled()
  })

  it('still reveals lines for explicit reveal events', () => {
    render(
      <CodeEditor
        {...defaultProps}
        initialCursorPosition={{ line: 2, col: 1 }}
        initialScrollTop={0}
      />
    )

    window.dispatchEvent(
      new CustomEvent(EDITOR_REVEAL_LINE_EVENT, {
        detail: {
          filePath: '/project/src/example.ts',
          lineNumber: 7,
          searchTerm: 'needle'
        }
      })
    )

    expect(mockScrollToLine).toHaveBeenCalledWith(7, 'needle')
    expect(mockScrollToLine).toHaveBeenCalledTimes(1)
    expect((global as PendingRevealLineWindow)[PENDING_REVEAL_LINE_GLOBAL]).toBeUndefined()
  })

  it('shows the loading overlay while the editor view is not ready', () => {
    vi.mocked(useCodeMirror).mockReturnValue({
      view: null,
      isReady: false,
      setContent: mockSetContent,
      flushPendingContent: vi.fn(),
      scrollToLine: mockScrollToLine,
      restoreViewState: mockRestoreViewState,
      getVisibleLineRange: vi.fn(() => null)
    })

    const { container, getByText } = render(<CodeEditor {...defaultProps} />)

    expect(getByText('Loading...')).toBeInTheDocument()
    const overlay = container.querySelector('[role="status"]')
    expect(overlay).not.toBeNull()
    expect(overlay?.getAttribute('aria-live')).toBe('polite')
  })
})

describe('CodeEditor TOC width persistence', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mobileRef.current = false
    tocSettingsRef.current = {
      isLoaded: true,
      loadFailed: false,
      settings: { isVisible: true, width: 280 },
      setWidth: vi.fn()
    }
    panelRef.onResize = undefined
    // react-resizable-panels synchronously notifies onResize from setLayout.
    // The editor's existing 60% minimum may constrain the requested TOC size.
    panelRef.setLayout.mockImplementation((layout: number[]) => {
      panelRef.onResize?.(Math.min(layout[1], 40), 50)
    })
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(600)
    vi.mocked(useCodeMirror).mockReturnValue({
      view: fakeView,
      isReady: true,
      setContent: mockSetContent,
      flushPendingContent: vi.fn(),
      scrollToLine: mockScrollToLine,
      restoreViewState: mockRestoreViewState,
      getVisibleLineRange: vi.fn(() => null)
    })
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('does not persist synchronous resize notifications from programmatic TOC layouts', () => {
    render(<CodeEditor {...defaultProps} language="markdown" />)

    expect(panelRef.setLayout).toHaveBeenCalled()
    expect(tocSettingsRef.current.setWidth).not.toHaveBeenCalled()
  })

  it('does not persist the initial mount resize notification without a previous size', () => {
    panelRef.setLayout.mockImplementation(() => {})
    render(<CodeEditor {...defaultProps} language="markdown" />)

    expect(panelRef.onResize).toBeDefined()
    act(() => panelRef.onResize?.(40, undefined))

    expect(tocSettingsRef.current.setWidth).not.toHaveBeenCalled()
  })

  it('still persists user resizing after applying the programmatic layout', () => {
    render(<CodeEditor {...defaultProps} language="markdown" />)

    expect(panelRef.onResize).toBeDefined()
    act(() => panelRef.onResize?.(35, 40))

    expect(tocSettingsRef.current.setWidth).toHaveBeenCalledExactlyOnceWith(210)
  })

  it('does not persist user resizing that rounds to the stored pixel width', () => {
    tocSettingsRef.current.settings.width = 210
    render(<CodeEditor {...defaultProps} language="markdown" />)

    expect(panelRef.onResize).toBeDefined()
    act(() => panelRef.onResize?.(35.01, 35))

    expect(tocSettingsRef.current.setWidth).not.toHaveBeenCalled()
  })
})

describe('CodeEditor mobile TOC hide', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mobileRef.current = false
    tocSettingsRef.current = {
      isLoaded: true,
      loadFailed: false,
      settings: { isVisible: true, width: 280 },
      setWidth: vi.fn()
    }
    vi.mocked(useCodeMirror).mockReturnValue({
      view: fakeView,
      isReady: true,
      setContent: mockSetContent,
      flushPendingContent: vi.fn(),
      scrollToLine: mockScrollToLine,
      restoreViewState: mockRestoreViewState,
      getVisibleLineRange: vi.fn(() => null)
    })
  })

  it('hides the TOC panel on mobile web shell so the editor gets full width', () => {
    // Desktop renders the TOC side-by-side, but that path exercises
    // `react-resizable-panels` whose 0-panel layout validator rejects jsdom's
    // zero clientWidth. The mobile branch is the CAP-5 fix under test: it
    // forces `canRenderToc=false` so the TOC panel never mounts.
    mobileRef.current = true
    const { container } = render(<CodeEditor {...defaultProps} language="markdown" />)

    expect(container.querySelector('[data-toc-panel]')).toBeNull()
  })
})
