import { render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { QueueSectionLabel } from './queue'

describe('QueueSectionLabel', () => {
  it('snaps the disclosure chevron under reduced motion', () => {
    const { container } = render(<QueueSectionLabel count={2} label="Queued" />)
    const chevron = container.querySelector('svg')
    expect(chevron).toHaveClass('transition-transform')
    expect(chevron).toHaveClass('motion-reduce:transition-none')
    expect(chevron).toHaveClass('motion-reduce:duration-0')
  })
})
