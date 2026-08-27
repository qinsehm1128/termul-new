import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import { ResizableRail } from './ResizableRail'

describe('ResizableRail', () => {
  beforeEach(() => {
    window.localStorage.clear()
  })

  it('resizes the left rail by dragging toward the right', () => {
    render(
      <ResizableRail
        side="left"
        storageKey="termul:sidebar-width"
        initial={240}
        min={180}
        max={420}
        resizeTitle="Resize sidebar"
        resizeLabel="Resize sidebar"
      >
        <div>sidebar</div>
      </ResizableRail>
    )

    const rail = screen.getByTestId('resizable-rail-left')
    expect(rail).toHaveStyle({ width: '240px' })
    fireEvent.pointerDown(screen.getByRole('separator'), { clientX: 240, button: 0 })
    fireEvent.pointerMove(document, { clientX: 300 })
    fireEvent.pointerUp(document)
    expect(rail).toHaveStyle({ width: '300px' })
  })

  it('resizes the right rail by dragging toward the left', () => {
    render(
      <ResizableRail
        side="right"
        storageKey="termul:file-explorer-width"
        initial={256}
        min={220}
        max={560}
        resizeTitle="Resize right sidebar"
        resizeLabel="Resize right sidebar"
      >
        <div>explorer</div>
      </ResizableRail>
    )

    const rail = screen.getByTestId('resizable-rail-right')
    fireEvent.pointerDown(screen.getByRole('separator'), { clientX: 800, button: 0 })
    fireEvent.pointerMove(document, { clientX: 740 })
    fireEvent.pointerUp(document)
    expect(rail).toHaveStyle({ width: '316px' })
  })
})
