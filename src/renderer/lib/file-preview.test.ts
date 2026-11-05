import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

const runtime = vi.hoisted(() => ({ desktop: true }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => runtime.desktop }))
vi.mock('@tauri-apps/api/core', () => ({ convertFileSrc: (path: string) => path }))

import { getDefaultViewMode } from '@/stores/editor-store'
import {
  getPreviewKind,
  getRasterImageMimeType,
  isDiskPreview,
  isTextPreview
} from './file-preview'

afterEach(() => {
  runtime.desktop = true
})

describe('raster preview routing', () => {
  it.each([true, false])('routes raster images to binary previews (desktop=%s)', (desktop) => {
    runtime.desktop = desktop
    for (const ext of [
      'png',
      'apng',
      'jpg',
      'jpeg',
      'jfif',
      'gif',
      'webp',
      'bmp',
      'ico',
      'avif',
      'tif',
      'tiff',
      'heic',
      'heif'
    ]) {
      const path = `/p/图片 #50%.${ext.toUpperCase()}`
      expect(getPreviewKind(path)).toBe('image')
      expect(getDefaultViewMode(path)).toBe('preview')
      expect(getRasterImageMimeType(path)).toMatch(/^image\//)
    }
    expect(isDiskPreview('image')).toBe(true)
    expect(isTextPreview('image')).toBe(false)
  })

  it('recognizes Windows raster paths and uses the appropriate codec MIME', () => {
    expect(getRasterImageMimeType('C:\\Pictures\\photo.JFIF')).toBe('image/jpeg')
    expect(getRasterImageMimeType('/p/animation.APNG')).toBe('image/png')
    expect(getRasterImageMimeType('/p/scan.tif')).toBe('image/tiff')
    expect(getRasterImageMimeType('/p/photo.heic')).toBe('image/heic')
    expect(getRasterImageMimeType('/p/photo.heif')).toBe('image/heif')
  })

  it('does not expand PDF, web HTML, audio or video binary support', () => {
    runtime.desktop = false
    for (const ext of ['pdf', 'html', 'htm', 'mp3', 'mp4', 'txt', 'constructor', '__proto__']) {
      expect(getPreviewKind(`/p/file.${ext}`)).toBeNull()
      expect(getRasterImageMimeType(`/p/file.${ext}`)).toBeNull()
    }
    expect(getPreviewKind('/p/art.svg')).toBe('svg')
    expect(getRasterImageMimeType('/p/art.svg')).toBeNull()
    runtime.desktop = true
    expect(getPreviewKind('/p/file.pdf')).toBeNull()
    expect(getPreviewKind('/p/file.mp3')).toBe('audio')
    expect(getPreviewKind('/p/file.mp4')).toBe('video')
  })
})

describe('html preview kind', () => {
  it('opens html rendered, as text with a source view', () => {
    for (const path of ['/p/index.html', '/p/old.HTM']) {
      expect(getPreviewKind(path)).toBe('html')
      expect(getDefaultViewMode(path)).toBe('preview')
    }
    expect(isTextPreview('html')).toBe(true)
    expect(isDiskPreview('html')).toBe(false)
  })

  it('opens html as plain source on the web, where the asset protocol is missing', () => {
    runtime.desktop = false
    expect(getPreviewKind('/p/index.html')).toBeNull()
    expect(getDefaultViewMode('/p/index.html')).toBe('code')
  })

  it('lets the desktop page frame files from the asset protocol', () => {
    const config = JSON.parse(
      readFileSync(join(process.cwd(), 'src-tauri/tauri.conf.json'), 'utf8')
    )
    const frameSrc = String(config.app.security.csp)
      .split(';')
      .map((directive) => directive.trim().split(/\s+/))
      .find(([name]) => name === 'frame-src')
    expect(frameSrc).toEqual(expect.arrayContaining(['asset:', 'http://asset.localhost']))
  })
})
