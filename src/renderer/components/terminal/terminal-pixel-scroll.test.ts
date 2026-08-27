import { afterEach, describe, expect, it, vi } from 'vitest'
import { logFrontendError } from '@/lib/log-api'
import { createXtermScreenFixture } from './__tests__/real-terminal-harness'
import {
  attachPixelSmoothScroll,
  computePixelOffset,
  formatPixelTranslateY,
  type PixelScrollTerminal
} from './terminal-pixel-scroll'

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn()
}))

afterEach(() => {
  vi.clearAllMocks()
})

describe('computePixelOffset', () => {
  it('is zero when scrollTop lands on a row', () => {
    expect(computePixelOffset(28, 14)).toBe(0)
  })

  it('keeps the leftover pixels between row snaps', () => {
    // scrollTop 20 → nearest row is 14 (row 1) or 28 (row 2); 20/14 ≈ 1.43 → 1
    expect(computePixelOffset(20, 14)).toBeCloseTo(14 - 20)
  })

  it('returns 0 for invalid cell height', () => {
    expect(computePixelOffset(20, 0)).toBe(0)
    expect(computePixelOffset(20, Number.NaN)).toBe(0)
  })
})

describe('formatPixelTranslateY', () => {
  it('clears the transform on a snapped row', () => {
    expect(formatPixelTranslateY(0, 2)).toBe('')
  })

  it('rounds to device pixels', () => {
    expect(formatPixelTranslateY(0.4, 2)).toBe('translate3d(0, 0.5px, 0)')
  })
})

describe('attachPixelSmoothScroll', () => {
  it('is a no-op when the private viewport is missing', () => {
    const handle = attachPixelSmoothScroll({} as PixelScrollTerminal)
    expect(handle.attached).toBe(false)
    expect(() => handle.dispose()).not.toThrow()
    expect(logFrontendError).not.toHaveBeenCalled()
  })

  it('logs when the core exists but the viewport surface is missing', () => {
    const handle = attachPixelSmoothScroll({
      _core: {}
    } as PixelScrollTerminal)
    expect(handle.attached).toBe(false)
    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({
        level: 'warn',
        source: 'ConnectedTerminal:pixel-smooth-scroll',
        message: 'unavailable: missing viewport or screen'
      })
    )
  })

  it('slides the screen by the discarded sub-row offset', () => {
    const screen = document.createElement('div')
    screen.className = 'xterm-screen'
    const host = document.createElement('div')
    host.appendChild(screen)

    let onScroll: ((event: { scrollTop: number }) => void) | undefined
    const scrollDispose = vi.fn()
    const bufferDispose = vi.fn()
    const resizeDispose = vi.fn()
    const dimensionsDispose = vi.fn()
    let onResize: (() => void) | undefined
    const terminal: PixelScrollTerminal = {
      element: host,
      onResize: (listener) => {
        onResize = listener
        return { dispose: resizeDispose }
      },
      onDimensionsChange: () => ({ dispose: dimensionsDispose }),
      _core: {
        _viewport: {
          _scrollableElement: {
            onScroll: (listener) => {
              onScroll = listener
              return { dispose: scrollDispose }
            }
          }
        },
        _renderService: {
          dimensions: { css: { cell: { height: 14 } } }
        },
        _bufferService: {
          buffers: {
            onBufferActivate: () => ({ dispose: bufferDispose })
          }
        }
      }
    }

    const attached = attachPixelSmoothScroll(terminal)
    expect(attached.attached).toBe(true)
    expect(onScroll).toBeTypeOf('function')
    expect(logFrontendError).not.toHaveBeenCalled()

    onScroll?.({ scrollTop: 20 })
    expect(screen.style.transform).toBe(
      formatPixelTranslateY(14 - 20, window.devicePixelRatio || 1)
    )
    expect(screen.style.willChange).toBe('transform')

    terminal._core!._renderService!.dimensions!.css!.cell!.height = 16
    onResize?.()
    expect(screen.style.transform).toBe(
      formatPixelTranslateY(16 - 20, window.devicePixelRatio || 1)
    )

    attached.dispose()
    expect(scrollDispose).toHaveBeenCalled()
    expect(bufferDispose).toHaveBeenCalled()
    expect(resizeDispose).toHaveBeenCalled()
    expect(dimensionsDispose).toHaveBeenCalled()
    expect(screen.style.transform).toBe('')
    expect(screen.style.willChange).toBe('')
  })

  it('clears the compositor hint when the offset snaps back to a whole row', () => {
    const screen = document.createElement('div')
    screen.className = 'xterm-screen'
    const host = document.createElement('div')
    host.appendChild(screen)

    let onScroll: ((event: { scrollTop: number }) => void) | undefined
    const terminal: PixelScrollTerminal = {
      element: host,
      _core: {
        _viewport: {
          _scrollableElement: {
            onScroll: (listener) => {
              onScroll = listener
              return { dispose() {} }
            }
          }
        },
        _renderService: {
          dimensions: { css: { cell: { height: 14 } } }
        }
      }
    }

    const attached = attachPixelSmoothScroll(terminal)

    onScroll?.({ scrollTop: 20 })
    expect(screen.style.willChange).toBe('transform')

    // 28 / 14 === 2 exactly, so the snapped offset is 0 and the transform clears.
    onScroll?.({ scrollTop: 28 })
    expect(screen.style.transform).toBe('')
    expect(screen.style.willChange).toBe('')

    attached.dispose()
  })

  it('leaves no compositor hint on a resize at rest', () => {
    const screen = document.createElement('div')
    screen.className = 'xterm-screen'
    const host = document.createElement('div')
    host.appendChild(screen)

    let onResize: (() => void) | undefined
    const terminal: PixelScrollTerminal = {
      element: host,
      onResize: (listener) => {
        onResize = listener
        return { dispose() {} }
      },
      _core: {
        _viewport: {
          _scrollableElement: {
            onScroll: () => ({ dispose() {} })
          }
        },
        _renderService: {
          dimensions: { css: { cell: { height: 14 } } }
        }
      }
    }

    const attached = attachPixelSmoothScroll(terminal)

    // No scroll has happened, so the resize path writes an offset of 0.
    onResize?.()
    expect(screen.style.willChange).toBe('')

    attached.dispose()
  })

  it('leaves .xterm-screen with no computed will-change in steady state', () => {
    const fixture = createXtermScreenFixture()
    let onScroll: ((event: { scrollTop: number }) => void) | undefined
    const attached = attachPixelSmoothScroll({
      screenElement: fixture.screen,
      _core: {
        _viewport: {
          _scrollableElement: {
            onScroll: (listener) => {
              onScroll = listener
              return { dispose() {} }
            }
          }
        },
        _renderService: {
          dimensions: { css: { cell: { height: 14 } } }
        }
      }
    })

    onScroll?.({ scrollTop: 20 })
    onScroll?.({ scrollTop: 28 })

    // Reads through the CSS cascade, so a promotion reintroduced from a
    // stylesheet is caught too — an inline assertion cannot see that.
    expect(fixture.readComputedWillChange()).toBe('')
    expect(fixture.readInlineWillChange()).toBe('')

    attached.dispose()
    fixture.dispose()
  })

  it('stops applying leftover offsets while the surface is hidden', () => {
    const screen = document.createElement('div')
    let onScroll: ((event: { scrollTop: number }) => void) | undefined
    const attached = attachPixelSmoothScroll({
      screenElement: screen,
      _core: {
        _viewport: {
          _scrollableElement: {
            onScroll: (listener) => {
              onScroll = listener
              return { dispose() {} }
            }
          }
        },
        _renderService: {
          dimensions: { css: { cell: { height: 14 } } }
        }
      }
    })

    onScroll?.({ scrollTop: 20 })
    expect(screen.style.transform).not.toBe('')

    attached.setEnabled(false)
    expect(screen.style.transform).toBe('')

    onScroll?.({ scrollTop: 27 })
    expect(screen.style.transform).toBe('')

    attached.setEnabled(true)
    attached.reset()
    expect(screen.style.transform).toBe('')
    attached.dispose()
  })

  it('uses the public screenElement when the host has no .xterm-screen child', () => {
    const screen = document.createElement('div')
    let onScroll: ((event: { scrollTop: number }) => void) | undefined
    const terminal: PixelScrollTerminal = {
      element: document.createElement('div'),
      screenElement: screen,
      _core: {
        _viewport: {
          _scrollableElement: {
            onScroll: (listener) => {
              onScroll = listener
              return { dispose() {} }
            }
          }
        },
        _renderService: {
          dimensions: { css: { cell: { height: 14 } } }
        }
      }
    }

    const attached = attachPixelSmoothScroll(terminal)
    expect(attached.attached).toBe(true)
    onScroll?.({ scrollTop: 20 })
    expect(screen.style.transform).toBe(
      formatPixelTranslateY(14 - 20, window.devicePixelRatio || 1)
    )
    attached.dispose()
  })
})
