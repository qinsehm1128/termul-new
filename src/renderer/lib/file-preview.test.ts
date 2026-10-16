import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

const runtime = vi.hoisted(() => ({ desktop: true }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => runtime.desktop }))
vi.mock('@tauri-apps/api/core', () => ({ convertFileSrc: (path: string) => path }))

import { getDefaultViewMode } from '@/stores/editor-store'
import { getPreviewKind, isDiskPreview, isTextPreview } from './file-preview'

afterEach(() => {
  runtime.desktop = true
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
