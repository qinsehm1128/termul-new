import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { FilePreview } from './FilePreview'

function image(): HTMLImageElement {
  const element = document.querySelector('img')
  if (!element) throw new Error('no image rendered')
  return element
}

describe('FilePreview', () => {
  afterEach(cleanup)

  it('fits an image to the pane and toggles actual size', () => {
    render(<FilePreview kind="image" src="asset://localhost/p/logo.png?v=1" />)

    expect(image()).toHaveAttribute('src', 'asset://localhost/p/logo.png?v=1')
    expect(image()).toHaveClass('max-w-full')

    fireEvent.click(screen.getByRole('button', { name: 'Actual size' }))
    expect(image()).toHaveClass('max-w-none')

    fireEvent.click(screen.getByRole('button', { name: 'Fit to window' }))
    expect(image()).toHaveClass('max-w-full')
  })

  it('shows the natural size once the image loads', () => {
    render(<FilePreview kind="image" src="asset://localhost/p/logo.png?v=1" />)
    Object.defineProperty(image(), 'naturalWidth', { value: 640 })
    Object.defineProperty(image(), 'naturalHeight', { value: 480 })

    fireEvent.load(image())

    expect(screen.getByText('640 × 480')).toBeInTheDocument()
  })

  it('says the file cannot be previewed when it fails to load', () => {
    const { rerender } = render(<FilePreview kind="image" src="asset://localhost/p/bad.png?v=1" />)

    fireEvent.error(image())
    expect(screen.getByText(/Cannot preview this file/)).toBeInTheDocument()

    rerender(<FilePreview kind="image" src="asset://localhost/p/bad.png?v=2" />)
    expect(screen.queryByText(/Cannot preview this file/)).not.toBeInTheDocument()
  })

  it('plays audio and video with native controls', () => {
    const { container, rerender } = render(<FilePreview kind="audio" src="asset://a.mp3" />)
    expect(container.querySelector('audio[controls]')).toHaveAttribute('src', 'asset://a.mp3')

    rerender(<FilePreview kind="video" src="asset://v.mp4" />)
    expect(container.querySelector('video[controls]')).toHaveAttribute('src', 'asset://v.mp4')
  })
})
