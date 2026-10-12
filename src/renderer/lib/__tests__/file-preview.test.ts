import { beforeEach, describe, expect, it, vi } from 'vitest'

const tauri = vi.hoisted(() => ({ inDesktop: true }))

vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => tauri.inDesktop
}))

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (path: string, protocol: string) => `${protocol}://localhost${encodeURI(path)}`
}))

import { getPreviewKind, isDiskPreview, previewSourceUrl, svgDataUrl } from '@/lib/file-preview'

describe('file preview kinds', () => {
  beforeEach(() => {
    tauri.inDesktop = true
  })

  it('previews images, svg, audio and video by extension, ignoring case', () => {
    expect(getPreviewKind('/p/logo.PNG')).toBe('image')
    expect(getPreviewKind('/p/photo.jpeg')).toBe('image')
    expect(getPreviewKind('C:\\p\\icon.svg')).toBe('svg')
    expect(getPreviewKind('/p/clip.mp3')).toBe('audio')
    expect(getPreviewKind('/p/demo.mov')).toBe('video')
  })

  it('opens everything else as text', () => {
    expect(getPreviewKind('/p/main.ts')).toBeNull()
    expect(getPreviewKind('/p/.png')).toBeNull()
    expect(getPreviewKind('/p/png')).toBeNull()
  })

  it('previews only svg in a web client, which has no asset protocol', () => {
    tauri.inDesktop = false
    expect(getPreviewKind('/p/logo.png')).toBeNull()
    expect(getPreviewKind('/p/clip.mp4')).toBeNull()
    expect(getPreviewKind('/p/icon.svg')).toBe('svg')
  })

  it('reads only non-svg kinds from disk', () => {
    expect(isDiskPreview('image')).toBe(true)
    expect(isDiskPreview('video')).toBe(true)
    expect(isDiskPreview('svg')).toBe(false)
    expect(isDiskPreview(null)).toBe(false)
  })

  it('versions the asset url so a changed file reloads', () => {
    expect(previewSourceUrl('/p/a b.png', 42)).toBe('asset://localhost/p/a%20b.png?v=42')
  })

  it('encodes svg source into an image data url', () => {
    expect(svgDataUrl('<svg a="#"/>')).toBe(
      'data:image/svg+xml;charset=utf-8,%3Csvg%20a%3D%22%23%22%2F%3E'
    )
  })
})
