import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { TabAgentMark, TabRunMark } from './tab-marks'

function runMark(container: HTMLElement): HTMLElement | null {
  return container.querySelector('[data-run-mark]')
}

describe('TabRunMark', () => {
  it('draws nothing for a terminal that is neither running nor active', () => {
    const { container } = render(<TabRunMark />)
    expect(runMark(container)).toBeNull()
  })

  it('draws a running mark for a live terminal', () => {
    const { container } = render(<TabRunMark running />)
    expect(runMark(container)).toHaveAttribute('data-run-mark', 'running')
  })

  it('prefers activity over merely running', () => {
    const { container } = render(<TabRunMark running activity />)
    expect(runMark(container)).toHaveAttribute('data-run-mark', 'activity')
  })

  it('prefers attention over everything else', () => {
    const { container } = render(<TabRunMark running activity attention />)
    expect(runMark(container)).toHaveAttribute('data-run-mark', 'attention')
  })

  it('is a round dot, distinguishing it from the agent star beside it', () => {
    const { container } = render(<TabRunMark running />)
    expect(runMark(container)?.className).toContain('rounded-full')
  })

  it('stays out of the accessibility tree — the tab label already names the terminal', () => {
    const { container } = render(<TabRunMark activity />)
    expect(runMark(container)).toHaveAttribute('aria-hidden')
  })
})

describe('TabAgentMark', () => {
  it('draws nothing when the state is unknown', () => {
    // The honesty invariant: no evidence must render as no mark. A dimmed star
    // would be read as idle, and idle is the one wrong answer that costs the
    // user something — they would never look at an agent that is stuck.
    const { container } = render(<TabAgentMark state="unknown" />)
    expect(container).toBeEmptyDOMElement()
  })

  it('draws nothing for a terminal with no agent state at all', () => {
    const { container } = render(<TabAgentMark />)
    expect(container).toBeEmptyDOMElement()
  })

  it.each(['working', 'blocked', 'idle'] as const)('draws a star for %s', (state) => {
    render(<TabAgentMark state={state} />)
    const mark = screen.getByRole('status')
    expect(mark).toHaveAttribute('data-agent-state', state)
    expect(mark).toHaveTextContent('✳')
  })

  it('gives each state its own label for assistive tech', () => {
    const labels = (['working', 'blocked', 'idle'] as const).map((state) => {
      const { unmount } = render(<TabAgentMark state={state} />)
      const label = screen.getByRole('status').getAttribute('aria-label')
      unmount()
      return label
    })
    expect(new Set(labels).size).toBe(3)
    expect(labels.every((label) => label != null && label.length > 0)).toBe(true)
  })

  it('animates only while working', () => {
    const { container: working } = render(<TabAgentMark state="working" />)
    expect(working.firstElementChild?.className).toContain('animate-pulse')

    for (const state of ['blocked', 'idle'] as const) {
      const { container } = render(<TabAgentMark state={state} />)
      expect(container.firstElementChild?.className).not.toContain('animate-pulse')
    }
  })

  it('gates the animation behind prefers-reduced-motion', () => {
    const { container } = render(<TabAgentMark state="working" />)
    expect(container.firstElementChild?.className).toContain('motion-safe:animate-pulse')
  })

  it('distinguishes the three states by colour as well as motion', () => {
    const tones = (['working', 'blocked', 'idle'] as const).map((state) => {
      const { container } = render(<TabAgentMark state={state} />)
      const className = container.firstElementChild?.className ?? ''
      return className
        .split(' ')
        .filter((token) => token.startsWith('text-'))
        .join(' ')
    })
    // Colour has to stand alone: with reduced motion on, it is the only channel
    // left, so two states sharing a tone would be indistinguishable.
    expect(new Set(tones).size).toBe(3)
  })
})
