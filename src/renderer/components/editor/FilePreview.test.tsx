import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  desktop: false,
  loadImage: vi.fn(),
  reveal: vi.fn(),
  openExternal: vi.fn()
}))
vi.mock('@/lib/file-preview-api', () => ({ loadImagePreview: mocks.loadImage }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('@/lib/tauri-opener-api', () => ({
  openerApi: { revealInFileManager: mocks.reveal, openWithExternalApp: mocks.openExternal }
}))

import { FilePreview } from './FilePreview'

function image(): HTMLImageElement {
  const element = document.querySelector('img')
  if (!element) throw new Error('no image rendered')
  return element
}

describe('FilePreview', () => {
  beforeEach(() => {
    mocks.desktop = false
    mocks.openExternal.mockReset()
    mocks.reveal.mockReset().mockResolvedValue({ success: true })
    mocks.loadImage.mockReset().mockImplementation(async (path: string) => ({
      success: true,
      data: { url: `blob:${path}`, dispose: vi.fn() }
    }))
  })
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
    expect(screen.getByText(/Unable to display this file/)).toBeInTheDocument()

    rerender(<FilePreview kind="image" src="asset://localhost/p/bad.png?v=2" />)
    expect(screen.queryByText(/Unable to display this file/)).not.toBeInTheDocument()
  })

  it('shows loading until decoded and clears dimensions and zoom when the source changes', () => {
    const { rerender } = render(<FilePreview kind="image" src="/preview/a.png" />)
    expect(screen.getByRole('status')).toHaveTextContent('Loading')
    Object.defineProperty(image(), 'naturalWidth', { value: 640 })
    Object.defineProperty(image(), 'naturalHeight', { value: 480 })
    fireEvent.load(image())
    fireEvent.click(screen.getByRole('button', { name: 'Actual size' }))
    expect(screen.queryByRole('status')).not.toBeInTheDocument()

    const oldImage = image()
    rerender(<FilePreview kind="image" src="/preview/b.png" />)
    expect(screen.queryByText('640 × 480')).not.toBeInTheDocument()
    expect(screen.getByRole('status')).toBeInTheDocument()
    expect(image()).toHaveClass('max-w-full')
    fireEvent.load(oldImage)
    fireEvent.error(oldImage)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.getByRole('status')).toBeInTheDocument()
  })

  it('retries the same source with a fresh decoder and keeps auth/version parameters', () => {
    render(<FilePreview kind="image" src="/fs/preview?token=secret&v=1" />)
    const first = image()
    fireEvent.error(first)
    fireEvent.click(screen.getByRole('button', { name: 'Retry preview' }))
    expect(image()).not.toBe(first)
    expect(image()).toHaveAttribute('src', '/fs/preview?token=secret&v=1&previewRetry=1')
    expect(screen.getByRole('status')).toBeInTheDocument()
    fireEvent.error(first)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()

    const second = image()
    fireEvent.error(second)
    fireEvent.click(screen.getByRole('button', { name: 'Retry preview' }))
    expect(image()).toHaveAttribute('src', '/fs/preview?token=secret&v=1&previewRetry=2')
    Object.defineProperty(image(), 'naturalWidth', { value: 120 })
    Object.defineProperty(image(), 'naturalHeight', { value: 80 })
    fireEvent.load(image())
    expect(screen.getByText('120 × 80')).toBeInTheDocument()
  })

  it('does not preserve a failed source when returning to it later', () => {
    const { rerender } = render(<FilePreview kind="image" src="/a.png" />)
    fireEvent.error(image())
    rerender(<FilePreview kind="image" src="/b.png" />)
    rerender(<FilePreview kind="image" src="/a.png" />)
    expect(image()).toHaveAttribute('src', '/a.png')
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('retries inline SVG without corrupting its data URL', () => {
    const src = 'data:image/svg+xml;charset=utf-8,%3Csvg%3E%3C%2Fsvg%3E'
    render(<FilePreview kind="svg" src={src} />)
    fireEvent.error(image())
    fireEvent.click(screen.getByRole('button', { name: 'Retry preview' }))
    expect(image()).toHaveAttribute('src', src)
  })

  it('reveals a decoder-unsupported original and surfaces file-manager failure without direct app launch', async () => {
    mocks.desktop = true
    mocks.reveal.mockResolvedValue({ success: false, error: 'File manager unavailable' })
    render(<FilePreview kind="image" src="asset://photo.heic" filePath="/p/photo.heic" />)
    await waitFor(() => expect(document.querySelector('img')).toBeInTheDocument())
    fireEvent.error(image())
    expect(screen.getByRole('alert')).toHaveTextContent('unsupported by this runtime')
    expect(screen.getByText(/then open it there with a compatible system app/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Show original in file manager' }))
    expect(mocks.reveal).toHaveBeenCalledWith('/p/photo.heic')
    expect(mocks.openExternal).not.toHaveBeenCalled()
    expect(await screen.findByText('File manager unavailable')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry preview' })).toBeInTheDocument()
  })

  it('can reveal the original successfully while retaining preview retry', async () => {
    mocks.desktop = true
    mocks.loadImage.mockResolvedValueOnce({ success: false, error: 'Unsupported codec' })
    render(<FilePreview kind="image" filePath="/p/photo.tiff" />)
    await screen.findByText('Unsupported codec')
    fireEvent.click(screen.getByRole('button', { name: 'Show original in file manager' }))
    await waitFor(() => expect(mocks.reveal).toHaveBeenCalledWith('/p/photo.tiff'))
    expect(mocks.openExternal).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: 'Retry preview' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Open in system app' })).not.toBeInTheDocument()
  })

  it('explains the host-system fallback on web without advertising unsupported local open', async () => {
    render(<FilePreview kind="image" src="/preview/photo.tiff" filePath="/p/photo.tiff" />)
    await waitFor(() => expect(document.querySelector('img')).toBeInTheDocument())
    fireEvent.error(image())
    expect(screen.getByText(/on the host computer/)).toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Show original in file manager' })
    ).not.toBeInTheDocument()
    expect(mocks.reveal).not.toHaveBeenCalled()
    expect(mocks.openExternal).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: 'Retry preview' })).toBeInTheDocument()
  })

  it('disposes stale async results without replacing the current file', async () => {
    const staleDispose = vi.fn()
    const currentDispose = vi.fn()
    let resolveOld!: (result: unknown) => void
    mocks.loadImage
      .mockReturnValueOnce(
        new Promise((resolve) => {
          resolveOld = resolve
        })
      )
      .mockResolvedValueOnce({
        success: true,
        data: { url: 'blob:current', dispose: currentDispose }
      })
    const { rerender, unmount } = render(
      <FilePreview kind="image" filePath="/old.png" version={1} />
    )
    const oldSignal = mocks.loadImage.mock.calls[0][1] as AbortSignal
    rerender(<FilePreview kind="image" filePath="/new.png" version={1} />)
    expect(oldSignal.aborted).toBe(true)
    await waitFor(() => expect(image()).toHaveAttribute('src', 'blob:current'))
    await act(async () =>
      resolveOld({ success: true, data: { url: 'blob:stale', dispose: staleDispose } })
    )
    expect(staleDispose).toHaveBeenCalledOnce()
    expect(image()).toHaveAttribute('src', 'blob:current')
    expect(currentDispose).not.toHaveBeenCalled()
    unmount()
    expect(currentDispose).toHaveBeenCalledOnce()
    expect((mocks.loadImage.mock.calls[1][1] as AbortSignal).aborted).toBe(true)
  })

  it('ignores a rejected old request after switching files', async () => {
    let rejectOld!: (reason: Error) => void
    mocks.loadImage.mockReturnValueOnce(
      new Promise((_resolve, reject) => {
        rejectOld = reject
      })
    )
    const { rerender } = render(<FilePreview kind="image" filePath="/old.png" />)
    rerender(<FilePreview kind="image" filePath="/new.png" />)
    await waitFor(() => expect(image()).toHaveAttribute('src', 'blob:/new.png'))
    await act(async () => rejectOld(new Error('Old request failed')))
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(image()).toHaveAttribute('src', 'blob:/new.png')
  })

  it('disposes a result that arrives after the preview unmounts', async () => {
    const dispose = vi.fn()
    let resolve!: (result: unknown) => void
    mocks.loadImage.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      })
    )
    const { unmount } = render(<FilePreview kind="image" filePath="/pending.png" />)
    unmount()
    expect((mocks.loadImage.mock.calls[0][1] as AbortSignal).aborted).toBe(true)
    await act(async () => resolve({ success: true, data: { url: 'blob:late', dispose } }))
    expect(dispose).toHaveBeenCalledOnce()
    expect(document.querySelector('img')).not.toBeInTheDocument()
  })

  it('re-reads bytes after decoder failure instead of retrying a revoked blob', async () => {
    const dispose = vi.fn()
    mocks.loadImage
      .mockResolvedValueOnce({ success: true, data: { url: 'blob:first', dispose } })
      .mockResolvedValueOnce({ success: true, data: { url: 'blob:second', dispose: vi.fn() } })
    render(<FilePreview kind="image" filePath="/photo.png" />)
    await waitFor(() => expect(image()).toHaveAttribute('src', 'blob:first'))
    const oldImage = image()
    fireEvent.error(oldImage)
    expect(dispose).toHaveBeenCalledOnce()
    fireEvent.click(screen.getByRole('button', { name: 'Retry preview' }))
    await waitFor(() => expect(image()).toHaveAttribute('src', 'blob:second'))
    fireEvent.error(oldImage)
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(mocks.loadImage).toHaveBeenCalledTimes(2)
  })

  it('shows the explicit transport error and retries loading the original path', async () => {
    mocks.loadImage.mockResolvedValueOnce({
      success: false,
      error: 'TIFF preview is not supported',
      code: 'UNSUPPORTED_IMAGE_FORMAT'
    })
    render(<FilePreview kind="image" filePath="/photo.tiff" />)
    expect(await screen.findByText('TIFF preview is not supported')).toBeInTheDocument()
    expect(document.querySelector('img')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Retry preview' }))
    await waitFor(() => expect(image()).toHaveAttribute('src', 'blob:/photo.tiff'))
    expect(mocks.loadImage).toHaveBeenLastCalledWith('/photo.tiff', expect.any(AbortSignal))
  })

  it('releases the previous version and clears dimensions while new bytes load', async () => {
    const dispose = vi.fn()
    mocks.loadImage.mockResolvedValueOnce({ success: true, data: { url: 'blob:v1', dispose } })
    const { rerender } = render(<FilePreview kind="image" filePath="/photo.png" version={1} />)
    await waitFor(() => expect(image()).toHaveAttribute('src', 'blob:v1'))
    Object.defineProperty(image(), 'naturalWidth', { value: 10 })
    Object.defineProperty(image(), 'naturalHeight', { value: 20 })
    fireEvent.load(image())
    expect(screen.getByText('10 × 20')).toBeInTheDocument()
    mocks.loadImage.mockReturnValueOnce(new Promise(() => {}))
    rerender(<FilePreview kind="image" filePath="/photo.png" version={2} />)
    expect(dispose).toHaveBeenCalledOnce()
    expect(screen.queryByText('10 × 20')).not.toBeInTheDocument()
    expect(screen.getByRole('status')).toBeInTheDocument()
    expect(document.querySelector('img')).not.toBeInTheDocument()
  })

  it('plays audio and video with native controls', () => {
    const { container, rerender } = render(<FilePreview kind="audio" src="asset://a.mp3" />)
    expect(container.querySelector('audio[controls]')).toHaveAttribute('src', 'asset://a.mp3')

    rerender(<FilePreview kind="video" src="asset://v.mp4" />)
    expect(container.querySelector('video[controls]')).toHaveAttribute('src', 'asset://v.mp4')
  })
})
