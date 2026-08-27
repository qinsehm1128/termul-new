import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PanelFade } from './panel-fade'

function mockReducedMotion(enabled: boolean): void {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    configurable: true,
    value: (query: string) => ({
      matches: enabled && query.includes('prefers-reduced-motion'),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false
    })
  })
}

describe('PanelFade', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    mockReducedMotion(false)
  })

  afterEach(() => {
    vi.useRealTimers()
    mockReducedMotion(false)
  })

  it('mounts open content instantly without scale motion', () => {
    render(
      <PanelFade open data-testid="sidebar-panel-fade">
        <div>Rail</div>
      </PanelFade>
    )

    const panel = screen.getByTestId('sidebar-panel-fade')
    expect(panel).toHaveAttribute('data-state', 'open')
    expect(panel).toHaveClass('duration-150', 'opacity-100', 'overflow-hidden')
    expect(panel.className).not.toMatch(/scale-/)
    expect(screen.getByText('Rail')).toBeInTheDocument()
  })

  it('keeps the panel mounted through a 150ms opacity exit, then unmounts', () => {
    const { rerender } = render(
      <PanelFade open>
        <div>Rail</div>
      </PanelFade>
    )

    rerender(
      <PanelFade open={false}>
        <div>Rail</div>
      </PanelFade>
    )

    const panel = screen.getByTestId('panel-fade')
    expect(panel).toHaveAttribute('data-state', 'closed')
    expect(panel).toHaveClass('opacity-0')
    expect(screen.getByText('Rail')).toBeInTheDocument()

    act(() => {
      vi.advanceTimersByTime(149)
    })
    expect(screen.getByText('Rail')).toBeInTheDocument()

    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(screen.queryByTestId('panel-fade')).not.toBeInTheDocument()
  })

  it('skips the fade duration when reduced motion is requested', () => {
    mockReducedMotion(true)

    const { rerender } = render(
      <PanelFade open>
        <div>Rail</div>
      </PanelFade>
    )

    expect(screen.getByTestId('panel-fade')).not.toHaveClass('duration-150')

    rerender(
      <PanelFade open={false}>
        <div>Rail</div>
      </PanelFade>
    )

    expect(screen.queryByTestId('panel-fade')).not.toBeInTheDocument()
  })
})
