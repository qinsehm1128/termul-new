import { runtimeT } from '@/i18n/runtime'
import { filesystemApi, systemApi } from '@/lib/api'
import { showOpenedFile } from '@/lib/file-open-target'
import { logFrontendError } from '@/lib/log-api'
import { useEditorStore } from '@/stores/editor-store'

/** Resolves relative terminal paths against the terminal cwd and active project root. */
export interface FilePathResolutionContext {
  cwd?: string
  workspaceCwd?: string
  projectRoot?: string
}

/** The result of resolving a terminal path candidate to an openable file. */
export type FilePathResolutionResult =
  | { ok: true; path: string }
  | { ok: false; reason: 'missing-context' | 'not-found' | 'not-file' | 'ambiguous' }

type OpenFilePathFailureReason =
  | Extract<FilePathResolutionResult, { ok: false }>['reason']
  | 'open-failed'

export type OpenFilePathResult =
  | { ok: true }
  | {
      ok: false
      reason: OpenFilePathFailureReason
      message: string
    }

/** A 1-based xterm link range on a single rendered line. */
export interface TerminalPathLinkRange {
  start: { x: number; y: number }
  end: { x: number; y: number }
}

/** A clickable file path link extracted from terminal output. */
export interface TerminalPathLink {
  range: TerminalPathLinkRange
  text: string
  activate: (event: MouseEvent, text: string) => void | Promise<void>
}

/**
 * One maximal run of characters that can belong to a single path token.
 *
 * Quotes, ASCII/CJK brackets and CJK/full-width punctuation all terminate a run.
 * That is what keeps `(src/App.tsx)` or `【docs/说明.md:12】` from swallowing
 * their wrapper, and what stops `。然后` from joining the token.
 */
const PATH_RUN_REGEX =
  /[^\s`"'<>|,;!?()[\]{}*=…—～、，。；：？！（）【】「」『』《》〈〉“”‘’\p{Cc}]+/gu

// Scheme prefixes never contain a dot, so `App.tsx:42` stays a path while
// `mailto:` / `file:` are still rejected.
const URI_SCHEME_REGEX = /^(?![A-Za-z]:[\\/])[a-zA-Z][a-zA-Z0-9+-]*:/
/** `example.com:8080/path` is a host and port, not a file path. `App.tsx:42` is not. */
const HOST_PORT_REGEX = /^[\w.-]+\.[A-Za-z]{2,}:\d+(?:[/?#]|$)/
/** Any `scheme://` inside a token marks it as part of a URL (`url=https://…` adjacency). */
const EMBEDDED_URL_SCHEME_REGEX = /[A-Za-z][A-Za-z0-9+.-]*:\/\//
const FRAGMENT_ONLY_REGEX = /^#/

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

const TRAILING_ASCII_PUNCTUATION_REGEX = /[.,;:!?]+$/
const TRAILING_CJK_PUNCTUATION_REGEX = /[、，。；：？！（）【】「」『』《》〈〉“”‘’…—～]+$/

/** CJK scripts (Han, kana, Hangul) that may legitimately sit inside a name. */
const CJK_LETTER_CLASS =
  '\\u3040-\\u30FF\\u3400-\\u4DBF\\u4E00-\\u9FFF\\uF900-\\uFAFF\\uAC00-\\uD7AF'
/** `src/App.tsx报错了` → cut right after `.tsx` (only when the extension is known). */
const EXTENSION_BEFORE_CJK_REGEX = new RegExp(`\\.([A-Za-z0-9_-]+)(?=[${CJK_LETTER_CLASS}])`, 'g')
/** `src/main.rs:12:5行` → cut right after `:12:5`. */
const POSITION_BEFORE_CJK_REGEX = new RegExp(`:\\d+(?::\\d+)?(?=[${CJK_LETTER_CLASS}])`, 'g')
/** `请查看src/App.tsx` or `修改package.json` → strip leading CJK prose before an ASCII path. */
const LEADING_CJK_BEFORE_ASCII_PATH_REGEX = new RegExp(
  `^([${CJK_LETTER_CLASS}]+)((?:~[\\\\/]|\\.{1,2}[\\\\/]|[A-Za-z]:[\\\\/]|[A-Za-z0-9_.-]+(?:[\\\\/]|\\.[A-Za-z0-9_-]+)).*)$`
)

/**
 * Extensions that make a slashless token a plausible file. Deliberately a
 * whitelist: `v1.2.3`, `1.0.0` and `window.location` must never qualify.
 */
const KNOWN_FILE_EXTENSIONS = new Set<string>([
  'astro',
  'bash',
  'bat',
  'c',
  'cc',
  'cfg',
  'cjs',
  'clj',
  'cmd',
  'conf',
  'cpp',
  'cs',
  'csproj',
  'css',
  'csv',
  'cxx',
  'dart',
  'diff',
  'env',
  'erl',
  'ex',
  'exs',
  'fish',
  'gif',
  'go',
  'gql',
  'gradle',
  'graphql',
  'h',
  'hpp',
  'hs',
  'htm',
  'html',
  'ico',
  'ini',
  'java',
  'jpeg',
  'jpg',
  'js',
  'json',
  'jsonc',
  'jsx',
  'jl',
  'kt',
  'kts',
  'less',
  'lock',
  'lua',
  'm',
  'markdown',
  'md',
  'mdx',
  'mjs',
  'mm',
  'mov',
  'mp3',
  'mp4',
  'nim',
  'otf',
  'patch',
  'pdf',
  'php',
  'pl',
  'plist',
  'png',
  'properties',
  'proto',
  'ps1',
  'r',
  'rb',
  'rs',
  'sass',
  'scala',
  'scss',
  'sh',
  'sln',
  'sql',
  'svelte',
  'svg',
  'swift',
  'tf',
  'tfvars',
  'toml',
  'ts',
  'tsv',
  'tsx',
  'ttf',
  'txt',
  'vue',
  'wasm',
  'wav',
  'webp',
  'woff',
  'woff2',
  'xcconfig',
  'xml',
  'yaml',
  'yml',
  'zig',
  'zsh'
])

/** Extensionless files that are still unmistakably files. */
const KNOWN_FILE_BASENAMES = new Set<string>([
  '.babelrc',
  '.editorconfig',
  '.env',
  '.eslintrc',
  '.gitattributes',
  '.gitignore',
  '.npmrc',
  '.nvmrc',
  '.prettierrc',
  'brewfile',
  'changelog',
  'cmakelists.txt',
  'dockerfile',
  'gemfile',
  'justfile',
  'license',
  'makefile',
  'notice',
  'procfile',
  'rakefile',
  'readme'
])

function normalizePathSeparators(value: string): string {
  return value.replace(/\\/g, '/')
}

function getPathExtension(name: string): string {
  const dot = name.lastIndexOf('.')
  return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1).toLowerCase() : ''
}

/** True for slashless tokens that are plausibly a file, e.g. `README.md` or `Makefile`. */
function isKnownBareFileName(value: string): boolean {
  if (!value || /[\\/]/.test(value) || /^[A-Za-z]:/.test(value)) return false
  if (KNOWN_FILE_BASENAMES.has(value.toLowerCase())) return true
  if (!/^[\p{L}\p{N}._-]+$/u.test(value)) return false
  const extension = getPathExtension(value)
  return extension.length > 0 && KNOWN_FILE_EXTENSIONS.has(extension)
}

/** True for `example.com:8080/...` but not for `App.tsx:42`, whose tail is a known extension. */
function looksLikeHostPort(text: string): boolean {
  const match = HOST_PORT_REGEX.exec(text)
  if (!match) return false
  const host = match[0].split(':')[0]
  return !KNOWN_FILE_EXTENSIONS.has(getPathExtension(host))
}

function looksLikeFilePath(text: string): boolean {
  if (!text || URI_SCHEME_REGEX.test(text) || FRAGMENT_ONLY_REGEX.test(text)) {
    return false
  }
  if (EMBEDDED_URL_SCHEME_REGEX.test(text)) {
    return false
  }
  if (looksLikeHostPort(text)) {
    return false
  }
  // `//example.com/x` is protocol-relative; a UNC path starts with a host, not a TLD.
  if (/^\/\/[^/]*\.[A-Za-z]{2,}/.test(text)) {
    return false
  }

  const withoutPosition = stripLineColumnSuffix(text)
  if (!withoutPosition || !/[\p{L}\p{N}]/u.test(withoutPosition)) {
    return false
  }

  if (withoutPosition.includes('/') || withoutPosition.includes('\\')) {
    return true
  }
  return isKnownBareFileName(withoutPosition)
}

function trimTrailingPathPunctuation(value: string): string {
  let result = value.replace(TRAILING_ASCII_PUNCTUATION_REGEX, '')

  for (const [opening, closing] of WRAPPER_PAIRS) {
    while (result.endsWith(closing) && !result.includes(opening)) {
      result = result.slice(0, -1)
    }
  }

  return result.replace(TRAILING_CJK_PUNCTUATION_REGEX, '')
}

/** Cuts a token where a known extension or position suffix runs into Chinese prose. */
function cutAtCjkBoundary(value: string): string {
  let cut = -1

  for (const match of value.matchAll(EXTENSION_BEFORE_CJK_REGEX)) {
    const ext = match[1]?.toLowerCase() ?? ''
    if (!KNOWN_FILE_EXTENSIONS.has(ext)) continue
    const end = (match.index ?? 0) + match[0].length
    if (end > 0 && (cut < 0 || end < cut)) cut = end
  }
  for (const match of value.matchAll(POSITION_BEFORE_CJK_REGEX)) {
    const end = (match.index ?? 0) + match[0].length
    if (end > 0 && (cut < 0 || end < cut)) cut = end
  }

  return cut > 0 && cut < value.length ? value.slice(0, cut) : value
}

function stripLeadingCjkProsePrefix(value: string): { text: string; offset: number } {
  const match = LEADING_CJK_BEFORE_ASCII_PATH_REGEX.exec(value)
  if (!match) return { text: value, offset: 0 }
  const [, prosePrefix, tail] = match
  if (!prosePrefix || !tail || !looksLikeFilePath(tail)) return { text: value, offset: 0 }
  return { text: tail, offset: prosePrefix.length }
}

function hasKnownPathTail(value: string): boolean {
  const lastSegment = value.split(/[\\/]/).filter(Boolean).at(-1) ?? ''
  const extension = getPathExtension(lastSegment)
  if (extension && KNOWN_FILE_EXTENSIONS.has(extension)) return true
  return KNOWN_FILE_BASENAMES.has(lastSegment.toLowerCase())
}

/**
 * A quoted body is one path only when the whole body is a path. Quoted prose
 * such as `"see src/App.tsx now"` is left for the normal token scan.
 */
function isWholeQuotedPath(inner: string): boolean {
  const trimmed = inner.trim()
  if (!trimmed || /[\r\n]/.test(trimmed)) return false
  if (FRAGMENT_ONLY_REGEX.test(trimmed)) return false

  if (/\s/.test(trimmed)) {
    // Spaces are legal in a path, but the body is one path only when the last
    // word is a known file and no earlier word is already a complete path.
    // Otherwise the quote holds prose that merely mentions one or more paths.
    const withoutPosition = stripLineColumnSuffix(trimmed)
    if (!/[\\/]/.test(withoutPosition)) return false

    const words = withoutPosition.split(/\s+/).filter(Boolean)
    const lastWord = words.at(-1) ?? ''
    if (!hasKnownPathTail(lastWord)) return false

    return !words
      .slice(0, -1)
      .some((word) => isKnownBareFileName(word) || (/[\\/]/.test(word) && hasKnownPathTail(word)))
  }

  const runs = trimmed.match(PATH_RUN_REGEX)
  if (runs?.length !== 1 || runs[0] !== trimmed) return false
  return looksLikeFilePath(trimmed)
}

function isUrlAdjacentPathMatch(line: string, start: number, _text: string): boolean {
  const prefix = line.slice(0, start)
  // The last whitespace-delimited token before the candidate.
  const tokenStart = prefix.search(/\S*$/)
  if (tokenStart < 0) return false

  const tokenPrefix = prefix.slice(tokenStart).replace(/^[`"'([{（【「『《〈“‘]+/, '')
  if (!tokenPrefix) return false

  return EMBEDDED_URL_SCHEME_REGEX.test(tokenPrefix) || URI_SCHEME_REGEX.test(tokenPrefix)
}

export interface FilePathMatch {
  text: string
  start: number
}

/** Finds file-like path tokens while excluding URL host/path fragments. */
export function findFilePathMatches(line: string): FilePathMatch[] {
  if (!line) return []

  const matches: FilePathMatch[] = []
  const wholeQuotedRanges: Array<[number, number]> = []

  // 1. Quoted segments. A quote span becomes one link only when its body is a
  // single path; quoted prose falls through to the token scan below so every
  // path inside it is still found.
  for (const match of line.matchAll(/(?<![\p{L}\p{N}_])(["'`])([^\r\n]*?)\1/gu)) {
    const start = match.index ?? 0
    const raw = match[0]
    const inner = match[2]
    if (!isWholeQuotedPath(inner)) continue
    if (isUrlAdjacentPathMatch(line, start, raw)) continue

    // A `:12:3` or `(12,3)` outside the quote keeps the wrapper in the range so
    // the click target stays contiguous with the visible text.
    const outsidePosition = /^(?::\d+(?::\d+)?|\(\s*\d+(?:,\s*\d+)?\))/.exec(
      line.slice(start + raw.length)
    )
    if (outsidePosition) {
      const text = `${raw}${outsidePosition[0]}`
      matches.push({ text, start })
      wholeQuotedRanges.push([start, start + text.length])
      continue
    }

    const trimmedInner = inner.trim()
    const innerOffset = inner.indexOf(trimmedInner)
    matches.push({ text: trimmedInner, start: start + 1 + Math.max(innerOffset, 0) })
    wholeQuotedRanges.push([start, start + raw.length])
  }

  // 2. Unquoted tokens. Punctuation and wrappers terminate a run, so offsets are
  // always the real index of the path itself. When a CJK boundary splits a run
  // (`详见src/App.tsx和src/main.rs`), resume scanning right after the cut so the
  // second path in the same run is not dropped.
  const runRegex = new RegExp(PATH_RUN_REGEX.source, PATH_RUN_REGEX.flags)
  for (let runMatch = runRegex.exec(line); runMatch !== null; runMatch = runRegex.exec(line)) {
    const runStart = runMatch.index
    const rawRun = runMatch[0]
    if (wholeQuotedRanges.some(([start, end]) => runStart >= start && runStart < end)) {
      continue
    }

    const trimmed = trimTrailingPathPunctuation(rawRun)
    if (!trimmed) continue

    const strippedLead = stripLeadingCjkProsePrefix(trimmed)
    const token = strippedLead.text
    const tokenStart = runStart + strippedLead.offset
    const textAfterCut = cutAtCjkBoundary(token)
    const cutWasApplied = textAfterCut.length < token.length

    const afterCandidate = cutWasApplied
      ? line.slice(tokenStart + textAfterCut.length)
      : line.slice(runStart + rawRun.length)
    const hasSeparator = textAfterCut.includes('/') || textAfterCut.includes('\\')
    const parenPosition =
      !cutWasApplied && trimmed === rawRun ? /^\(\s*\d+(?:,\s*\d+)?\)/.exec(afterCandidate) : null
    // `console.log(` and `res.json(` are calls, not files. A bare token may keep
    // its parenthesis only when that is an explicit `(line,column)` position.
    if (
      !cutWasApplied &&
      trimmed === rawRun &&
      !hasSeparator &&
      afterCandidate.startsWith('(') &&
      !parenPosition?.[0].includes(',')
    ) {
      continue
    }

    let text = textAfterCut
    if (parenPosition) text += parenPosition[0]
    if (cutWasApplied) {
      runRegex.lastIndex = tokenStart + textAfterCut.length
    }
    if (!looksLikeFilePath(text)) continue
    if (isUrlAdjacentPathMatch(line, tokenStart, text)) continue

    matches.push({ text, start: tokenStart })
  }

  matches.sort((a, b) => a.start - b.start || b.text.length - a.text.length)

  const deduped: FilePathMatch[] = []
  for (const match of matches) {
    const overlaps = deduped.some(
      (kept) =>
        match.start < kept.start + kept.text.length && kept.start < match.start + match.text.length
    )
    if (!overlaps) deduped.push(match)
  }
  return deduped
}

/** Builds xterm link ranges for already-matched path tokens. */
export function buildPathLinksFromMatches(
  matches: FilePathMatch[],
  lineNumber: number,
  onActivate: (event: MouseEvent, text: string) => void | Promise<void>
): TerminalPathLink[] {
  return matches.map(({ text, start }) => ({
    range: {
      start: { x: start + 1, y: lineNumber },
      end: { x: start + text.length, y: lineNumber }
    },
    text,
    activate: onActivate
  }))
}

/** Extracts file-like links from a rendered terminal line for Ctrl/Cmd+Click activation. */
// Range x-coordinates are based on JavaScript string indices. Wide or combining
// terminal characters can render to different cell widths, and this module only
// receives plain text, so it cannot accurately map visual cells here.
export function buildTerminalPathLinks(
  line: string,
  lineNumber: number,
  onActivate: (event: MouseEvent, text: string) => void | Promise<void>
): TerminalPathLink[] {
  return buildPathLinksFromMatches(findFilePathMatches(line), lineNumber, onActivate)
}

function normalizeAbsolutePath(value: string): string {
  const normalized = normalizePathSeparators(value)

  if (normalized.startsWith('//')) {
    return `//${normalized.slice(2).replace(/\/+/g, '/')}`
  }

  return normalized.replace(/\/+/g, '/')
}

function isAbsolutePath(value: string): boolean {
  const normalized = normalizePathSeparators(value)
  return (
    normalized.startsWith('//') || normalized.startsWith('/') || /^[a-zA-Z]:\//.test(normalized)
  )
}

function joinPath(base: string, candidate: string): string {
  const normalizedBase = normalizePathSeparators(base).replace(/\/+$/, '')
  const normalizedCandidate = normalizePathSeparators(candidate).replace(/^\/+/, '')
  return normalizeAbsolutePath(`${normalizedBase}/${normalizedCandidate}`)
}

function canonicalizeAbsolutePath(value: string): string | null {
  const normalized = normalizeAbsolutePath(value)

  let root = '/'
  let remainder = normalized

  if (/^[a-zA-Z]:\//.test(normalized)) {
    root = `${normalized.slice(0, 2)}/`
    remainder = normalized.slice(3)
  } else if (normalized.startsWith('//')) {
    const segments = normalized.split('/').filter(Boolean)
    if (segments.length < 2) {
      return null
    }
    root = `//${segments[0]}/${segments[1]}`
    remainder = segments.slice(2).join('/')
  } else if (normalized.startsWith('/')) {
    remainder = normalized.slice(1)
  } else {
    return null
  }

  const parts = remainder.split('/').filter(Boolean)
  const resolvedParts: string[] = []

  for (const part of parts) {
    if (part === '.') {
      continue
    }

    if (part === '..') {
      if (resolvedParts.length === 0) {
        return null
      }
      resolvedParts.pop()
      continue
    }

    resolvedParts.push(part)
  }

  const joinedParts = resolvedParts.join('/')
  const canonicalPath = joinedParts
    ? root === '/'
      ? `/${joinedParts}`
      : normalizeAbsolutePath(`${root}/${joinedParts}`)
    : root

  return canonicalPath
}

function shouldCompareCaseInsensitively(path: string, root: string): boolean {
  return (
    /^[a-zA-Z]:\//.test(path) ||
    /^[a-zA-Z]:\//.test(root) ||
    path.startsWith('//') ||
    root.startsWith('//')
  )
}

function isPathWithinRoot(path: string, root: string): boolean {
  const comparablePath = shouldCompareCaseInsensitively(path, root) ? path.toLowerCase() : path
  const comparableRoot = shouldCompareCaseInsensitively(path, root) ? root.toLowerCase() : root

  if (comparablePath === comparableRoot) {
    return true
  }

  if (comparableRoot === '/') {
    return comparablePath.startsWith('/')
  }

  return comparablePath.startsWith(`${comparableRoot}/`)
}

function getAllowedRoots(context: FilePathResolutionContext): string[] {
  const roots = [context.cwd, context.workspaceCwd, context.projectRoot]
    .map((value) => (value ? canonicalizeAbsolutePath(value) : null))
    .filter((value): value is string => Boolean(value))

  return [...new Set(roots)]
}

function isHomeRelativePath(value: string): boolean {
  return value === '~' || value.startsWith('~/') || value.startsWith('~\\')
}

function expandHomePath(candidate: string, homeDirectory: string | null): string | null {
  if (!homeDirectory) return null
  const relative = candidate.replace(/^~[\\/]?/, '')
  return canonicalizeAbsolutePath(joinPath(homeDirectory, relative))
}

/**
 * Relative-looking variants of a token. `git diff` output prefixes paths with
 * `a/` and `b/`, so the unprefixed form is tried as a fallback.
 */
function buildCandidateVariants(candidate: string): string[] {
  const variants = [candidate]
  const stripped = candidate.replace(/^([ab])\//, '')
  if (stripped !== candidate && stripped.includes('/')) {
    variants.push(stripped)
  }
  return variants
}

function buildResolutionCandidates(
  candidate: string,
  roots: string[],
  homeDirectory: string | null = null
): string[] {
  const paths: string[] = []
  const add = (value: string | null): void => {
    if (!value || paths.includes(value)) return
    if (roots.length > 0 && !roots.some((root) => isPathWithinRoot(value, root))) return
    paths.push(value)
  }

  for (const variant of buildCandidateVariants(candidate)) {
    if (isHomeRelativePath(variant)) {
      // `~/…` is an explicit user path: expand it and keep it even outside the
      // project roots, because the terminal text named that file directly.
      const expanded = expandHomePath(variant, homeDirectory)
      if (expanded && !paths.includes(expanded)) paths.push(expanded)
      continue
    }

    if (isAbsolutePath(variant)) {
      const absoluteCandidate = canonicalizeAbsolutePath(variant)
      if (absoluteCandidate && roots.some((root) => isPathWithinRoot(absoluteCandidate, root))) {
        add(absoluteCandidate)
      }
      continue
    }

    for (const root of roots) {
      const resolvedCandidate = canonicalizeAbsolutePath(joinPath(root, variant))
      // A `../` hop may leave the starting root but still land inside another
      // allowed root (cwd, workspace cwd, or project root); accept any of them.
      add(resolvedCandidate)
    }
  }

  return paths
}

function needsHomeExpansion(candidate: string): boolean {
  return buildCandidateVariants(candidate).some(isHomeRelativePath)
}

/** Removes a single layer of wrapping quotes, brackets, or backticks from a path token. */
export function trimWrappedPath(value: string): string {
  const result = value.trim()

  for (const [start, end] of WRAPPER_PAIRS) {
    if (
      result.startsWith(start) &&
      result.endsWith(end) &&
      result.length >= start.length + end.length
    ) {
      return result.slice(start.length, result.length - end.length).trim()
    }
  }

  return result
}

/** `:12`, `:12:3`, `#L12`, `#L12C3` and `(12,3)` position suffixes. */
const TRAILING_POSITION_REGEX = /(?::\d+(?::\d+)?|#L\d+(?:C\d+)?|\(\s*\d+(?:,\s*\d+)?\))$/i

/** Removes trailing position suffixes from a path token. */
export function stripLineColumnSuffix(value: string): string {
  return value.replace(TRAILING_POSITION_REGEX, '')
}

function parseLineColumnSuffix(value: string): { line?: number; column?: number } {
  const match = value.match(/(?::(\d+)(?::(\d+))?|#L(\d+)(?:C(\d+))?|\(\s*(\d+)(?:,\s*(\d+))?\))$/i)
  if (!match) {
    return {}
  }

  const rawLine = match[1] ?? match[3] ?? match[5]
  const rawColumn = match[2] ?? match[4] ?? match[6]
  const line = Number.parseInt(rawLine, 10)
  const parsedColumn = rawColumn ? Number.parseInt(rawColumn, 10) : null

  return {
    line: Number.isFinite(line) && line > 0 ? line : undefined,
    column:
      parsedColumn !== null && Number.isFinite(parsedColumn) && parsedColumn > 0
        ? parsedColumn
        : undefined
  }
}

/** Normalizes a terminal token into a file path candidate before filesystem resolution. */
export function extractPathCandidate(value: string): string {
  let result = trimWrappedPath(value)
  // `src/App.tsx:12:3:` ends with a dangling colon that hides the position suffix.
  result = result.replace(/:+$/, '')
  result = stripLineColumnSuffix(result)
  result = trimWrappedPath(result)
  return result
}

const MAX_SUFFIX_SEARCH_HITS = 256
const SUFFIX_SEARCH_TIMEOUT_MS = 2500
let suffixSearchSequence = 0

/** A partial search can never establish uniqueness. Never search home or filesystem roots. */
function canSearchRoot(root: string): boolean {
  return (
    root !== '/' &&
    root !== '/root' &&
    root !== '/var/root' &&
    root !== '/private/var/root' &&
    !/^[A-Za-z]:\/$/.test(root) &&
    !/^\/(?:Users|home)(?:\/[^/]+)?\/?$/.test(root) &&
    !/^[A-Za-z]:\/(?:Users|Documents and Settings)(?:\/[^/]+)?\/?$/i.test(root)
  )
}

async function getSearchHomeDirectory(): Promise<string | null> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      systemApi
        .getHomeDirectory()
        .then((result) => (result.success ? canonicalizeAbsolutePath(result.data) : null)),
      new Promise<null>((resolve) => {
        timer = setTimeout(() => resolve(null), SUFFIX_SEARCH_TIMEOUT_MS)
      })
    ])
  } catch {
    return null
  } finally {
    if (timer) clearTimeout(timer)
  }
}

async function searchExactSuffix(root: string, suffix: string): Promise<string[] | null> {
  const searchId = `file-link-${Date.now()}-${++suffixSearchSequence}`
  const matches = new Set<string>()
  let count = 0
  let settled = false
  let cleanupBatch = (): void => {}
  let cleanupDone = (): void => {}
  let timer: ReturnType<typeof setTimeout> | undefined
  return new Promise((resolve) => {
    const finish = (result: string[] | null, cancel = false): void => {
      if (settled) return
      settled = true
      if (timer) clearTimeout(timer)
      cleanupBatch()
      cleanupDone()
      if (cancel) {
        void filesystemApi.searchFileNamesStreamCancel(searchId).catch(() => {
          void logFrontendError({
            source: 'file-path-links',
            message: 'Suffix search cancellation failed',
            level: 'warn'
          })
        })
      }
      resolve(result)
    }
    try {
      cleanupBatch = filesystemApi.onSearchFileNamesBatch((event) => {
        if (settled || event.searchId !== searchId) return
        count += event.files.length
        if (event.truncated || count > MAX_SUFFIX_SEARCH_HITS) {
          void logFrontendError({
            source: 'file-path-links',
            message: 'Suffix search bound reached',
            level: 'warn'
          })
          finish(null, true)
          return
        }
        for (const hit of event.files) {
          const path =
            canonicalizeAbsolutePath(hit.path) ?? canonicalizeAbsolutePath(joinPath(root, hit.path))
          if (path && isPathWithinRoot(path, root) && path.endsWith(`/${suffix}`)) {
            matches.add(path)
          }
        }
        if (matches.size > 1) finish([...matches], true)
      })
      cleanupDone = filesystemApi.onSearchFileNamesDone((event) => {
        if (settled || event.searchId !== searchId) return
        if (event.truncated || event.error || event.code) {
          void logFrontendError({
            source: 'file-path-links',
            message: 'Suffix search incomplete',
            level: 'warn'
          })
          finish(null)
        } else {
          finish([...matches])
        }
      })
      timer = setTimeout(() => {
        void logFrontendError({
          source: 'file-path-links',
          message: 'Suffix search timed out',
          level: 'warn'
        })
        finish(null, true)
      }, SUFFIX_SEARCH_TIMEOUT_MS)
      void filesystemApi
        .searchFileNamesStreamStart(searchId, root, root, suffix.split('/').at(-1)!, false)
        .then((result) => {
          if (!result.success) finish(null, true)
        })
        .catch(() => {
          void logFrontendError({
            source: 'file-path-links',
            message: 'Suffix search failed',
            level: 'warn'
          })
          finish(null, true)
        })
    } catch {
      void logFrontendError({
        source: 'file-path-links',
        message: 'Suffix search subscription failed',
        level: 'warn'
      })
      finish(null, true)
    }
  })
}

async function resolveUniqueSuffix(
  candidate: string,
  roots: string[]
): Promise<FilePathResolutionResult> {
  const suffix = normalizePathSeparators(candidate).replace(/^\.\//, '')
  // Explicit absolute and parent-relative paths must never be reinterpreted as a suffix.
  if (
    isAbsolutePath(candidate) ||
    candidate.startsWith('~') ||
    suffix.split('/').some((part) => !part || part === '.' || part === '..')
  ) {
    return { ok: false, reason: 'not-found' }
  }
  const home = await getSearchHomeDirectory()
  if (!home) {
    void logFrontendError({
      source: 'file-path-links',
      level: 'warn',
      message: 'Suffix search home boundary unavailable'
    })
    return { ok: false, reason: 'not-found' }
  }
  const safeRoots = roots.filter((root) => canSearchRoot(root) && !isPathWithinRoot(home, root))
  const searchRoots = safeRoots.filter(
    (root) => !safeRoots.some((other) => other !== root && isPathWithinRoot(root, other))
  )
  if (!searchRoots.length || searchRoots.length > 3) return { ok: false, reason: 'not-found' }
  const results = await Promise.all(searchRoots.map((root) => searchExactSuffix(root, suffix)))
  if (results.some((result) => result === null)) return { ok: false, reason: 'not-found' }
  const matches = [...new Set(results.flatMap((result) => result ?? []))]
  if (matches.length > 1) {
    void logFrontendError({
      source: 'file-path-links',
      message: 'Ambiguous path suffix rejected',
      level: 'warn'
    })
    return { ok: false, reason: 'ambiguous' }
  }
  if (matches.length === 1) {
    try {
      const info = await filesystemApi.getFileInfo(matches[0])
      if (info.success && info.data.type === 'file') return { ok: true, path: matches[0] }
    } catch {
      void logFrontendError({ source: 'file-path-links', message: 'Suffix target stat failed' })
    }
  }
  return { ok: false, reason: 'not-found' }
}

function normalizeProbeMatchKey(candidate: string): string {
  let normalized = stripLineColumnSuffix(candidate.trim()).replace(/\\/g, '/')
  while (normalized.startsWith('./')) {
    normalized = normalized.slice(2)
  }
  return normalizeAbsolutePath(normalized)
}

/**
 * Batch existence check through the host. Returns `null` when the capability is
 * unavailable or failed, so callers fall back to `getFileInfo` / suffix search.
 */
async function resolveExistingPathCandidates(
  absoluteCandidates: string[],
  roots: string[]
): Promise<string | null> {
  const resolveExistingPaths = filesystemApi.resolveExistingPaths
  if (typeof resolveExistingPaths !== 'function' || absoluteCandidates.length === 0) {
    return null
  }

  try {
    const result = await resolveExistingPaths({
      candidates: absoluteCandidates,
      roots,
      expandHome: true
    })
    if (!result.success) return null

    const hitByNormalized = new Map<string, string>()
    for (const entry of result.data.hits) {
      hitByNormalized.set(entry.candidate, entry.resolvedPath)
      hitByNormalized.set(normalizeProbeMatchKey(entry.candidate), entry.resolvedPath)
    }
    for (const candidate of absoluteCandidates) {
      const resolved =
        hitByNormalized.get(candidate) ?? hitByNormalized.get(normalizeProbeMatchKey(candidate))
      if (resolved) return resolved
    }
    return null
  } catch {
    void logFrontendError({
      source: 'file-path-links',
      message: 'Batch path resolution failed',
      level: 'warn'
    })
    return null
  }
}

/** Resolves a terminal path token to an existing file using terminal and project context. */
export async function resolveFilePathCandidate(
  rawCandidate: string,
  context: FilePathResolutionContext
): Promise<FilePathResolutionResult> {
  const candidate = extractPathCandidate(rawCandidate)

  if (!candidate) {
    return { ok: false, reason: 'not-found' }
  }

  if (candidate.endsWith('/') || candidate.endsWith('\\')) {
    return { ok: false, reason: 'not-file' }
  }

  const allowedRoots = getAllowedRoots(context)

  if (allowedRoots.length === 0) {
    return { ok: false, reason: 'missing-context' }
  }

  const homeDirectory = needsHomeExpansion(candidate) ? await getSearchHomeDirectory() : null
  const absoluteCandidates = buildResolutionCandidates(candidate, allowedRoots, homeDirectory)

  if (absoluteCandidates.length === 0) {
    return { ok: false, reason: 'not-found' }
  }

  const batchHit = await resolveExistingPathCandidates(absoluteCandidates, allowedRoots)
  if (batchHit) {
    return { ok: true, path: batchHit }
  }

  const infoResults = await Promise.all(
    absoluteCandidates.map(async (absolutePath) => {
      try {
        return { absolutePath, infoResult: await filesystemApi.getFileInfo(absolutePath) }
      } catch {
        void logFrontendError({ source: 'file-path-links', message: 'Exact path stat failed' })
        return { absolutePath, infoResult: null }
      }
    })
  )

  let sawDirectoryCandidate = false

  for (const { absolutePath, infoResult } of infoResults) {
    if (!infoResult?.success) {
      continue
    }

    if (infoResult.data.type === 'file') {
      return { ok: true, path: absolutePath }
    }

    sawDirectoryCandidate = true
  }

  return sawDirectoryCandidate
    ? { ok: false, reason: 'not-file' }
    : resolveUniqueSuffix(candidate, allowedRoots)
}

function getErrorMessage(
  rawCandidate: string,
  reason: OpenFilePathFailureReason,
  details?: string
): string {
  const candidate = extractPathCandidate(rawCandidate)

  switch (reason) {
    case 'missing-context':
      return runtimeT(
        'terminal',
        'links.missingContext',
        'No project or working directory found; set a project/cwd to open paths: {{path}}',
        { path: candidate }
      )
    case 'ambiguous':
      return runtimeT('terminal', 'links.ambiguousPath', 'Multiple files match this path.')
    case 'not-file':
      return runtimeT(
        'terminal',
        'links.pathIsDirectory',
        'Path is a directory, not a file: {{path}}',
        { path: candidate }
      )
    case 'not-found':
      return runtimeT('terminal', 'links.fileNotFound', 'File not found: {{path}}', {
        path: candidate
      })
    case 'open-failed':
      return details
        ? runtimeT(
            'terminal',
            'links.openFileFailedWithDetails',
            'Failed to open file: {{path}} ({{details}})',
            { path: candidate, details }
          )
        : runtimeT('terminal', 'links.openFileFailedForPath', 'Failed to open file: {{path}}', {
            path: candidate
          })
  }
}

/** Opens a resolved file path from terminal output and adds it to the editor workspace. */
export async function openFilePathFromTerminal(
  rawCandidate: string,
  context: FilePathResolutionContext
): Promise<OpenFilePathResult> {
  const resolution = await resolveFilePathCandidate(rawCandidate, context)

  if (!resolution.ok) {
    void logFrontendError({
      source: 'file-path-links',
      level: 'warn',
      message: `File link resolution rejected: ${resolution.reason}`
    })
    return {
      ok: false,
      reason: resolution.reason,
      message: getErrorMessage(rawCandidate, resolution.reason)
    }
  }

  const wrappedCandidate = trimWrappedPath(rawCandidate)
  const position = parseLineColumnSuffix(wrappedCandidate)

  try {
    await useEditorStore.getState().openFile(resolution.path)

    if (position.line) {
      useEditorStore
        .getState()
        .updateCursorPosition(resolution.path, position.line, position.column ?? 1)
    }

    showOpenedFile(resolution.path, { source: 'terminal' })
    return { ok: true }
  } catch (error) {
    void logFrontendError({ source: 'file-path-links', message: 'File link editor open failed' })
    const details = error instanceof Error ? error.message : String(error)
    return {
      ok: false,
      reason: 'open-failed',
      message: getErrorMessage(rawCandidate, 'open-failed', details)
    }
  }
}

const PATH_PROBE_CACHE_TTL_MS = 30_000
const PATH_PROBE_CACHE_MAX_ENTRIES = 256

interface PathProbeCacheEntry {
  expiresAt: number
  exists: boolean
}

/** Bounded TTL/LRU cache for terminal link existence probes. */
const pathProbeCache = new Map<string, PathProbeCacheEntry>()

/** Clears the terminal link existence cache (tests and project switches). */
export function clearPathProbeCache(): void {
  pathProbeCache.clear()
}

function readPathProbe(key: string): boolean | undefined {
  const entry = pathProbeCache.get(key)
  if (!entry) return undefined
  if (entry.expiresAt <= Date.now()) {
    pathProbeCache.delete(key)
    return undefined
  }
  // Refresh recency so the capped map behaves as an LRU.
  pathProbeCache.delete(key)
  pathProbeCache.set(key, entry)
  return entry.exists
}

function writePathProbe(key: string, exists: boolean): void {
  if (pathProbeCache.size >= PATH_PROBE_CACHE_MAX_ENTRIES) {
    const oldest = pathProbeCache.keys().next().value
    if (oldest !== undefined) pathProbeCache.delete(oldest)
  }
  pathProbeCache.set(key, { exists, expiresAt: Date.now() + PATH_PROBE_CACHE_TTL_MS })
}

/**
 * A purely syntactic match that still needs filesystem proof before it becomes
 * clickable: slashless file names and hierarchical tokens without a known file
 * extension (`application/json`, `12/31`, `yes/no`, `feat/login`).
 */
export function isAmbiguousPathMatch(text: string): boolean {
  const candidate = extractPathCandidate(text)
  if (!candidate || isAbsolutePath(candidate) || isHomeRelativePath(candidate)) return false

  const hasSeparator = candidate.includes('/') || candidate.includes('\\')
  if (!hasSeparator) return true

  const lastSegment = candidate.split(/[\\/]/).filter(Boolean).at(-1) ?? ''
  const extension = getPathExtension(lastSegment)
  return extension.length === 0 || !KNOWN_FILE_EXTENSIONS.has(extension)
}

/**
 * Keeps only ambiguous matches that exist on disk. Uses the batch host API plus
 * a TTL/LRU cache. When the capability is missing nothing is confirmed, so the
 * caller keeps showing just the unambiguous links.
 */
export async function resolveExistingTerminalPathMatches(
  matches: FilePathMatch[],
  context: FilePathResolutionContext
): Promise<FilePathMatch[]> {
  const resolveExistingPaths = filesystemApi.resolveExistingPaths
  if (typeof resolveExistingPaths !== 'function' || matches.length === 0) return []

  const roots = getAllowedRoots(context)
  if (roots.length === 0) return []

  const rootKey = roots.join('\u0000')
  const confirmed: FilePathMatch[] = []
  const pending: Array<{ match: FilePathMatch; candidate: string }> = []

  for (const match of matches) {
    const candidate = extractPathCandidate(match.text)
    if (!candidate) continue
    const cached = readPathProbe(`${rootKey}\u0000${candidate}`)
    if (cached === true) confirmed.push(match)
    else if (cached === undefined) pending.push({ match, candidate })
  }

  if (pending.length === 0) return confirmed.sort((a, b) => a.start - b.start)

  try {
    const result = await resolveExistingPaths({
      candidates: [...new Set(pending.map((entry) => entry.candidate))],
      roots,
      expandHome: true
    })
    if (result.success) {
      const hits = new Set<string>()
      for (const hit of result.data.hits) {
        hits.add(hit.candidate)
        hits.add(normalizeProbeMatchKey(hit.candidate))
      }
      for (const { match, candidate } of pending) {
        const exists = hits.has(candidate) || hits.has(normalizeProbeMatchKey(candidate))
        writePathProbe(`${rootKey}\u0000${candidate}`, exists)
        if (exists) confirmed.push(match)
      }
    }
  } catch {
    void logFrontendError({
      source: 'file-path-links',
      message: 'Terminal link existence probe failed',
      level: 'warn'
    })
  }

  return confirmed.sort((a, b) => a.start - b.start)
}
