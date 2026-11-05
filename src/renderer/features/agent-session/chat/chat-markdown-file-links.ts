import { brandCanonical } from '@shared/brand'
import { findFilePathMatches } from '@/lib/file-path-links'

/**
 * Href scheme for a file path the renderer turned into a link.
 *
 * The prefix ends up inside persisted chat markdown, so a flip that only knew
 * the new spelling would break every file link in a transcript written before
 * it. Writes use {@link FILE_PATH_LINK_PREFIX}; reads accept the legacy prefix
 * too, and never write it back.
 */
export const FILE_PATH_LINK_PREFIX = `${brandCanonical().displayName.toLowerCase()}-file-path:`

/** Every prefix a stored href may carry, most-current first. */
const READABLE_FILE_PATH_PREFIXES = [FILE_PATH_LINK_PREFIX, 'termul-file-path:']

type MarkdownNode = {
  type: string
  value?: string
  url?: string
  title?: string | null
  children?: MarkdownNode[]
}

export function filePathHref(path: string): string {
  return `${FILE_PATH_LINK_PREFIX}${encodeURIComponent(path)}`
}

function splitTextNode(node: MarkdownNode): MarkdownNode[] {
  const value = node.value ?? ''
  const matches = findFilePathMatches(value)
  if (matches.length === 0) return [node]

  const result: MarkdownNode[] = []
  let cursor = 0

  for (const match of matches) {
    if (match.start > cursor) {
      result.push({ type: 'text', value: value.slice(cursor, match.start) })
    }
    result.push({
      type: 'link',
      title: null,
      url: filePathHref(match.text),
      children: [{ type: 'text', value: match.text }]
    })
    cursor = match.start + match.text.length
  }

  if (cursor < value.length) {
    result.push({ type: 'text', value: value.slice(cursor) })
  }

  return result
}

/** Explicit Markdown destinations may contain spaces that prose token detection cannot capture. */
function relativeFilePathFromHref(href: string): string | null {
  let path: string
  try {
    path = decodeURIComponent(href)
  } catch {
    return null
  }

  // Keep browser destinations out of the file resolver, including encoded schemes
  // and protocol-relative URLs. Fragments and queries retain their URL semantics.
  if (
    !path ||
    /^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(path) ||
    /^[\\/]/.test(path) ||
    /[?#]/.test(path) ||
    Array.from(path).some(
      (character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127
    )
  ) {
    return null
  }

  const withoutPosition = path.replace(/:\d+(?::\d+)?$/, '')
  if (withoutPosition.includes(':')) return null
  return /[\\/]/.test(withoutPosition) || /\.[\w-]+$/.test(withoutPosition) ? path : null
}

function transformNode(node: MarkdownNode): MarkdownNode[] {
  if (node.type === 'text') return splitTextNode(node)
  if (node.type === 'link') {
    const path = node.url ? relativeFilePathFromHref(node.url) : null
    if (path) node.url = filePathHref(path)
    return [node]
  }
  if (node.type === 'code') return [node]

  if (!node.children) return [node]
  node.children = node.children.flatMap(transformNode)
  return [node]
}

/** Streamdown remark plugin that routes prose paths and explicit relative links to the editor. */
export function remarkFilePathLinks(): (tree: MarkdownNode) => void {
  return (tree) => {
    if (tree.children) tree.children = tree.children.flatMap(transformNode)
  }
}

export function filePathFromHref(href: string | undefined): string | null {
  const prefix = READABLE_FILE_PATH_PREFIXES.find((candidate) => href?.startsWith(candidate))
  if (!href || !prefix) return null
  try {
    return decodeURIComponent(href.slice(prefix.length))
  } catch {
    return null
  }
}
