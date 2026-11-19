export interface TerminalUrlLinkRange {
  start: { x: number; y: number }
  end: { x: number; y: number }
}

export interface TerminalUrlLink {
  range: TerminalUrlLinkRange
  text: string
  activate: (event: MouseEvent, text: string) => void | Promise<void>
}

const WRAPPER_PAIRS: Array<[string, string]> = [
  ['`', '`'],
  ['"', '"'],
  ["'", "'"],
  ['(', ')'],
  ['[', ']'],
  ['（', '）'],
  ['【', '】'],
  ['「', '」'],
  ['『', '』'],
  ['《', '》'],
  ['〈', '〉'],
  ['“', '”'],
  ['‘', '’']
]

const TRAILING_PUNCTUATION = /[\]),.;:!?、，。；：？！（）【】「」『』《》〈〉“”‘’…—～]+$/
/** CJK and full-width punctuation terminates a URL just like whitespace does. */
const URL_CANDIDATE_REGEX =
  /(?:https?:\/\/|[a-zA-Z][a-zA-Z0-9+.-]*:)[^\s<>{}"'`、，。；：？！（）【】「」『』《》〈〉“”‘’]+/g

function trimWrapped(value: string): string {
  const result = value.trim()
  for (const [start, end] of WRAPPER_PAIRS) {
    if (
      result.startsWith(start) &&
      result.endsWith(end) &&
      result.length > start.length + end.length
    ) {
      return result.slice(start.length, result.length - end.length).trim()
    }
  }
  return result
}

function trimLeadingWrapper(value: string): string {
  const result = value.trim()
  for (const [start] of WRAPPER_PAIRS) {
    if (result.startsWith(start) && result.length > start.length) {
      return result.slice(start.length).trim()
    }
  }
  return result
}

export function sanitizeTerminalUrlToken(value: string): string {
  const withoutTrailing = value.trim().replace(TRAILING_PUNCTUATION, '')
  return trimLeadingWrapper(trimWrapped(withoutTrailing))
}

export function isSupportedTerminalUrl(value: string): boolean {
  try {
    const parsed = new URL(value)
    return parsed.protocol === 'http:' || parsed.protocol === 'https:'
  } catch {
    return false
  }
}

/**
 * Schemes a terminal may legitimately print without `//`. Anything else that
 * looks like `ext:12:3` or `App.tsx:42` is a path with a position suffix.
 */
const KNOWN_SCHEME_ONLY_PROTOCOLS = new Set([
  'bitcoin',
  'ftp',
  'ftps',
  'git',
  'irc',
  'ircs',
  'magnet',
  'mailto',
  'matrix',
  'obsidian',
  'sftp',
  'slack',
  'ssh',
  'tel',
  'vscode',
  'zulip'
])

const SCHEME_PREFIX_REGEX = /^([a-zA-Z][a-zA-Z0-9+.-]*):(.*)$/

export function isTerminalUrlCandidate(value: string): boolean {
  if (/^[a-zA-Z]:[\\/]/.test(value)) {
    return false
  }

  if (!value.includes('://')) {
    const schemeMatch = SCHEME_PREFIX_REGEX.exec(value)
    if (schemeMatch) {
      const [, scheme, rest] = schemeMatch
      // `ts:12:3` / `App.tsx:42` are file paths with position suffixes, and a
      // dotted "scheme" is a filename, not a protocol.
      if (scheme.includes('.') || /^\d+(?::\d+)?(?:[,;)]|$)/.test(rest)) {
        return false
      }
      return KNOWN_SCHEME_ONLY_PROTOCOLS.has(scheme.toLowerCase())
    }
  }

  try {
    // Accept absolute URL-like candidates so unsupported schemes can be rejected with feedback on activation.
    // Supported schemes are validated separately via isSupportedTerminalUrl.
    new URL(value)
    return true
  } catch {
    return false
  }
}

export function buildTerminalUrlLinks(
  line: string,
  lineNumber: number,
  onActivate: (event: MouseEvent, text: string) => void | Promise<void>
): TerminalUrlLink[] {
  const links: TerminalUrlLink[] = []

  for (const match of line.matchAll(URL_CANDIDATE_REGEX)) {
    const raw = match[0]
    const start = match.index ?? -1
    if (start < 0) continue

    const cleaned = sanitizeTerminalUrlToken(raw)
    if (!isTerminalUrlCandidate(cleaned)) {
      continue
    }

    // Sanitizing may strip a leading wrapper, so anchor the range on where the
    // clean URL actually starts instead of assuming it begins at the raw match.
    const offsetInRaw = Math.max(raw.indexOf(cleaned), 0)
    const cleanedStart = start + offsetInRaw

    links.push({
      range: {
        start: { x: cleanedStart + 1, y: lineNumber },
        end: { x: cleanedStart + cleaned.length, y: lineNumber }
      },
      text: cleaned,
      activate: onActivate
    })
  }

  return links
}
