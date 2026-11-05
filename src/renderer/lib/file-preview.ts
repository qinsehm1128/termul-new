import { convertFileSrc } from '@tauri-apps/api/core'
import { isTauriContext } from '@/lib/tauri-runtime'

/**
 * Files the editor shows as a preview instead of text. `svg` and `html` are
 * text too, so they keep their source buffer and can switch to the code view;
 * the others have no text buffer. Raster images use binary preview transport;
 * desktop audio/video continue to use the asset protocol.
 */
export type PreviewKind = 'image' | 'svg' | 'html' | 'audio' | 'video'

const PREVIEW_KIND_BY_EXTENSION: Record<string, PreviewKind> = {
  png: 'image',
  jpg: 'image',
  jpeg: 'image',
  jfif: 'image',
  tif: 'image',
  tiff: 'image',
  heic: 'image',
  heif: 'image',
  gif: 'image',
  webp: 'image',
  bmp: 'image',
  ico: 'image',
  avif: 'image',
  apng: 'image',
  svg: 'svg',
  html: 'html',
  htm: 'html',
  mp3: 'audio',
  wav: 'audio',
  ogg: 'audio',
  oga: 'audio',
  m4a: 'audio',
  aac: 'audio',
  flac: 'audio',
  opus: 'audio',
  mp4: 'video',
  m4v: 'video',
  webm: 'video',
  mov: 'video',
  ogv: 'video'
}

function extensionOf(filePath: string): string {
  const name = filePath.split(/[\\/]/).pop() || filePath
  const dotIndex = name.lastIndexOf('.')
  return dotIndex > 0 ? name.slice(dotIndex + 1).toLowerCase() : ''
}

/**
 * Preview kind for a path, or null when it opens as text. Raster transport is
 * available in web and desktop. HTML and audio/video still require the desktop
 * asset protocol; SVG renders from its existing text source in either mode.
 * A raster extension remains an image even when its codec cannot be decoded:
 * the preview reports a decode error instead of opening binary bytes as text.
 */
export function getPreviewKind(filePath: string): PreviewKind | null {
  const extension = extensionOf(filePath)
  const kind = Object.hasOwn(PREVIEW_KIND_BY_EXTENSION, extension)
    ? PREVIEW_KIND_BY_EXTENSION[extension]
    : null
  if (kind && kind !== 'svg' && kind !== 'image' && !isTauriContext()) return null
  return kind
}

// Raster-only MIME allowlist. SVG and HTML must never be loaded by binary
// preview transport, including when a server responds with an unexpected type.
const RASTER_MIME_BY_EXTENSION: Record<string, string> = {
  png: 'image/png',
  apng: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  jfif: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  bmp: 'image/bmp',
  ico: 'image/x-icon',
  avif: 'image/avif',
  tif: 'image/tiff',
  tiff: 'image/tiff',
  heic: 'image/heic',
  heif: 'image/heif'
}

export function getRasterImageMimeType(filePath: string): string | null {
  const extension = extensionOf(filePath)
  return Object.hasOwn(RASTER_MIME_BY_EXTENSION, extension)
    ? RASTER_MIME_BY_EXTENSION[extension]
    : null
}

/** True for previewable kinds that are text, with a source view to switch to. */
export function isTextPreview(kind: PreviewKind | null): boolean {
  return kind === 'svg' || kind === 'html'
}

/** True for kinds that have no text buffer at all. */
export function isDiskPreview(kind: PreviewKind | null): boolean {
  return kind !== null && !isTextPreview(kind)
}

/** Asset URL for a disk-backed preview; `version` busts the cache after a change on disk. */
export function previewSourceUrl(filePath: string, version: number): string {
  return `${convertFileSrc(filePath, 'asset')}?v=${version}`
}

/** Data URL that renders SVG source as an image, so scripts inside it never run. */
export function svgDataUrl(source: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(source)}`
}
