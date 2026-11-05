import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useTocSettingsStore } from '@/stores/toc-settings-store'
import { DEFAULT_TOC_SETTINGS } from '@/types/settings'
import { CodeEditor } from './CodeEditor'

// Vitest's Node condition otherwise selects the library's server build, whose
// layout effects are disabled. Use the unmodified browser implementation, not
// stand-ins for PanelGroup, Panel, PanelResizeHandle, or their imperative API.
vi.mock('react-resizable-panels', async () => {
  const { createRequire } = await import('node:module')
  const { dirname, join } = await import('node:path')
  const require = createRequire(import.meta.url)
  return vi.importActual(
    join(
      dirname(require.resolve('react-resizable-panels/package.json')),
      'dist/react-resizable-panels.browser.development.esm.js'
    )
  )
})

vi.mock('@/hooks/use-codemirror', () => ({
  useCodeMirror: () => ({
    view: null,
    isReady: true,
    setContent: vi.fn(),
    flushPendingContent: vi.fn(),
    scrollToLine: vi.fn(),
    restoreViewState: vi.fn()
  })
}))

vi.mock('@/hooks/use-mobile-web-shell', () => ({
  useMobileWebShell: () => false
}))

vi.mock('./TocPanel', () => ({
  TocPanel: () => <div data-testid="toc-content" />
}))

class LayoutResizeObserver {
  static instances = new Set<LayoutResizeObserver>()
  private elements = new Set<Element>()

  constructor(private callback: ResizeObserverCallback) {
    LayoutResizeObserver.instances.add(this)
  }

  observe(element: Element): void {
    this.elements.add(element)
  }

  unobserve(element: Element): void {
    this.elements.delete(element)
  }

  disconnect(): void {
    this.elements.clear()
    LayoutResizeObserver.instances.delete(this)
  }

  static notify(): void {
    for (const observer of LayoutResizeObserver.instances) {
      const entries = Array.from(observer.elements, (target) => ({
        target,
        contentRect: { width: target.clientWidth }
      })) as ResizeObserverEntry[]
      observer.callback(entries, observer as unknown as ResizeObserver)
    }
  }
}

const props = {
  filePath: '/project/readme.md',
  content: '# Heading',
  language: 'markdown',
  isVisible: true,
  onChange: vi.fn(),
  onCursorChange: vi.fn(),
  onScrollChange: vi.fn()
}

function getTocPanel(): HTMLElement {
  const panel = screen.getByTestId('toc-content').closest<HTMLElement>('[data-panel]')
  if (!panel) throw new Error('Expected a real react-resizable-panels Panel')
  return panel
}

describe('CodeEditor TOC layout with real resizable panels', () => {
  let containerWidth = 1000
  const initialState = useTocSettingsStore.getState()

  beforeEach(() => {
    containerWidth = 1000
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(() => containerWidth)
    vi.stubGlobal('ResizeObserver', LayoutResizeObserver)
    useTocSettingsStore.setState({
      settings: { ...DEFAULT_TOC_SETTINGS, isVisible: true, width: 280 },
      isLoaded: true,
      loadFailed: false
    })
  })

  afterEach(() => {
    cleanup()
    useTocSettingsStore.setState(initialState, true)
    LayoutResizeObserver.instances.clear()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  it('keeps the saved 280px width when the container grows from 1000px to 2000px without dragging', () => {
    const widths: number[] = []
    const unsubscribe = useTocSettingsStore.subscribe((state, previous) => {
      if (state.settings.width !== previous.settings.width) widths.push(state.settings.width)
    })

    try {
      render(<CodeEditor {...props} />)
      expect(getTocPanel()).toHaveAttribute('data-panel-size', '28.0')
      expect(useTocSettingsStore.getState().settings.width).toBe(280)
      expect(LayoutResizeObserver.instances.size).toBeGreaterThan(0)

      // New pixel bounds become 7.5%-17.5%. The real Panel's constraint
      // reconciliation emits onResize before the editor's saved-layout effect;
      // an imperative setLayout mock misses this path (RV-002).
      act(() => {
        containerWidth = 2000
        LayoutResizeObserver.notify()
      })

      expect(useTocSettingsStore.getState().settings.width).toBe(280)
      expect(widths).toEqual([]) // In particular, never persist the transient 350px clamp.
      expect(getTocPanel()).toHaveAttribute('data-panel-size', '14.0')
    } finally {
      unsubscribe()
    }
  })

  it('still persists a keyboard resize through the real separator', () => {
    render(<CodeEditor {...props} />)
    const handle = screen.getByRole('separator')
    expect(getTocPanel()).toHaveAttribute('data-panel-size', '28.0')

    fireEvent.keyDown(handle, { key: 'ArrowRight' })
    fireEvent.keyUp(handle, { key: 'ArrowRight' })

    expect(getTocPanel()).toHaveAttribute('data-panel-size', '18.0')
    expect(useTocSettingsStore.getState().settings.width).toBe(180)
  })
})
