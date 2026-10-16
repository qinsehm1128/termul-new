import { convertFileSrc } from '@tauri-apps/api/core'
import { isTauriContext } from '@/lib/tauri-runtime'

/**
 * Files the editor shows as a preview instead of text. `svg` and `html` are
 * text too, so they keep their source buffer and can switch to the code view;
 * the others have no text buffer and are loaded straight from disk through the
 * asset protocol.
 */
export type PreviewKind = 'image' | 'svg' | 'html' | 'audio' | 'video'

const PREVIEW_KIND_BY_EXTENSION: Record<string, PreviewKind> = {
  png: 'image',
  jpg: 'image',
  jpeg: 'image',
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
 * Preview kind for a path, or null when it opens as text. Disk-backed kinds
 * and HTML (rendered from the saved file, so its relative assets resolve) need
 * the desktop asset protocol, so a web client only previews SVG, which renders
 * from the text it already has.
 */
export function getPreviewKind(filePath: string): PreviewKind | null {
  const kind = PREVIEW_KIND_BY_EXTENSION[extensionOf(filePath)] ?? null
  if (kind && kind !== 'svg' && !isTauriContext()) return null
  return kind
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
