import type { IDisposable, Terminal } from '@xterm/xterm'
import { logFrontendError } from '@/lib/log-api'

/**
 * Pixel-smooth trackpad scrolling for xterm 6.
 *
 * xterm already tracks fractional `scrollTop` (`forceIntegerValues: false`)
 * but snaps the canvas to whole rows in `_handleScroll`. Upstream PR
 * https://github.com/xtermjs/xterm.js/pull/6039 keeps the leftover as a
 * compositor `translateY`. That patch is not in 6.1-beta.216; we apply the
 * same offset from the viewport scroll stream.
 */

export interface PixelScrollSurface {
  onScroll?: (listener: (event: { scrollTop: number }) => void) => IDisposable | (() => void)
}

export interface PixelScrollViewport {
  _scrollableElement?: PixelScrollSurface
}

export interface PixelScrollCore {
  _viewport?: PixelScrollViewport
  _renderService?: {
    dimensions?: { css?: { cell?: { height?: number } } }
  }
  _bufferService?: {
    buffers?: {
      onBufferActivate?: (listener: () => void) => IDisposable | (() => void)
    }
  }
}

export interface PixelScrollTerminal {
  element?: HTMLElement | null
  screenElement?: HTMLElement | null
  onResize?: (listener: () => void) => IDisposable | (() => void)
  onDimensionsChange?: (listener: () => void) => IDisposable | (() => void)
  _core?: PixelScrollCore
}

export interface PixelSmoothScrollHandle extends IDisposable {
  attached: boolean
  reset: () => void
  setEnabled: (enabled: boolean) => void
}

const LOG_SOURCE = 'ConnectedTerminal:pixel-smooth-scroll'

function reportFailure(message: string, error?: unknown): void {
  const suffix =
    error instanceof Error ? error.message : error !== undefined ? String(error) : undefined
  void logFrontendError({
    level: 'warn',
    source: LOG_SOURCE,
    message: suffix ? `${message}: ${suffix}` : message,
    stack: error instanceof Error ? error.stack : undefined
  })
}

export function computePixelOffset(scrollTop: number, cellHeight: number): number {
  if (!(cellHeight > 0) || !Number.isFinite(scrollTop)) return 0
  const newRow = Math.round(scrollTop / cellHeight)
  return newRow * cellHeight - scrollTop
}

export function formatPixelTranslateY(offset: number, devicePixelRatio: number): string {
  const dpr = devicePixelRatio > 0 ? devicePixelRatio : 1
  const snapped = Math.round(offset * dpr) / dpr
  if (snapped === 0) return ''
  return `translate3d(0, ${snapped}px, 0)`
}

function disposeListener(disposable: IDisposable | (() => void) | undefined): void {
  if (!disposable) return
  if (typeof disposable === 'function') {
    disposable()
    return
  }
  disposable.dispose()
}

function resolveCore(terminal: PixelScrollTerminal): PixelScrollCore | undefined {
  if (terminal._core) return terminal._core
  const maybeCore = terminal as PixelScrollCore
  if (maybeCore._viewport || maybeCore._renderService || maybeCore._bufferService) {
    return maybeCore
  }
  return undefined
}

function resolveScreen(terminal: PixelScrollTerminal): HTMLElement | undefined {
  if (terminal.screenElement instanceof HTMLElement) return terminal.screenElement
  const found = terminal.element?.querySelector('.xterm-screen')
  return found instanceof HTMLElement ? found : undefined
}

function applyOffset(screen: HTMLElement, offset: number): void {
  const dpr = screen.ownerDocument.defaultView?.devicePixelRatio ?? 1
  const transform = formatPixelTranslateY(offset, dpr)
  screen.style.transform = transform
  // Promote only while a sub-row offset is actually applied. Writing this
  // unconditionally left `.xterm-screen` — the direct parent of the WebGL
  // canvas — permanently promoted to its own compositor layer in the steady
  // state where nothing is animating.
  screen.style.willChange = transform === '' ? '' : 'transform'
}

function clearOffset(screen: HTMLElement): void {
  screen.style.transform = ''
  screen.style.willChange = ''
}

/**
 * Follow xterm's scrollable and slide `.xterm-screen` by the discarded
 * sub-row pixels. No-ops if the private viewport surface is missing.
 */
export function attachPixelSmoothScroll(
  terminal: Terminal | PixelScrollTerminal
): PixelSmoothScrollHandle {
  const core = resolveCore(terminal as PixelScrollTerminal)
  const scrollable = core?._viewport?._scrollableElement
  const screen = resolveScreen(terminal as PixelScrollTerminal)
  if (!core) {
    return { attached: false, reset() {}, setEnabled() {}, dispose() {} }
  }
  if (!scrollable?.onScroll || !screen) {
    reportFailure('unavailable: missing viewport or screen')
    return { attached: false, reset() {}, setEnabled() {}, dispose() {} }
  }

  const readCellHeight = (): number => core._renderService?.dimensions?.css?.cell?.height ?? 0

  let lastScrollTop = 0
  let enabled = true

  const applyFromScrollTop = (scrollTop: number): void => {
    lastScrollTop = scrollTop
    if (!enabled) return
    try {
      applyOffset(screen, computePixelOffset(scrollTop, readCellHeight()))
    } catch (error) {
      reportFailure('apply offset failed', error)
    }
  }

  const reset = (): void => {
    lastScrollTop = 0
    try {
      clearOffset(screen)
    } catch (error) {
      reportFailure('reset offset failed', error)
    }
  }

  const scrollDisposable = scrollable.onScroll((event) => {
    applyFromScrollTop(event.scrollTop)
  })
  const bufferDisposable = core._bufferService?.buffers?.onBufferActivate?.(reset)
  const resizeDisposable = (terminal as PixelScrollTerminal).onResize?.(() => {
    applyFromScrollTop(lastScrollTop)
  })
  const dimensionsDisposable = (terminal as PixelScrollTerminal).onDimensionsChange?.(() => {
    applyFromScrollTop(lastScrollTop)
  })

  reset()

  return {
    attached: true,
    reset,
    setEnabled(next: boolean): void {
      enabled = next
      if (!next) reset()
    },
    dispose(): void {
      enabled = false
      disposeListener(scrollDisposable)
      disposeListener(bufferDisposable)
      disposeListener(resizeDisposable)
      disposeListener(dimensionsDisposable)
      reset()
    }
  }
}
