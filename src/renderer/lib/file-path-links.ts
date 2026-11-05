import { runtimeT } from '@/i18n/runtime'
import { filesystemApi, systemApi } from '@/lib/api'
import { logFrontendError } from '@/lib/log-api'
import { useEditorStore } from '@/stores/editor-store'
import { useWorkspaceStore } from '@/stores/workspace-store'

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

const FILE_PATH_LINK_REGEX =
  /(?:\.{1,2}[\\/]|[A-Za-z]:[\\/]|\\\\|\/)?(?:[^\s\\/:*?"<>|`]+[\\/])+[^\s\\/:*?"<>|`]+(?:\.[A-Za-z0-9_-]+)?(?::\d+(?::\d+)?)?/g

const URI_SCHEME_REGEX = /^(?![A-Za-z]:[\\/])[a-zA-Z][a-zA-Z0-9+.-]*:/

const WRAPPER_PAIRS: Array<[string, string]> = [
  ['`', '`'],
  ['"', '"'],
  ["'", "'"],
  ['(', ')'],
  ['[', ']']
]

function normalizePathSeparators(value: string): string {
  return value.replace(/\\/g, '/')
}

function looksLikeFilePath(text: string): boolean {
  if (URI_SCHEME_REGEX.test(text) || text.startsWith('#')) {
    return false
  }

  return text.includes('/') || text.includes('\\') || /^[A-Za-z]:/.test(text)
}

function trimTrailingPathPunctuation(value: string): string {
  let result = value.replace(/[.,;!?]+$/, '')

  for (const [opening, closing] of WRAPPER_PAIRS) {
    while (result.endsWith(closing) && !result.includes(opening)) {
      result = result.slice(0, -1)
    }
  }

  return result
}

function isUrlAdjacentPathMatch(line: string, start: number, text: string): boolean {
  const tokenStart = line.slice(0, start).search(/\S+$/)
  if (tokenStart < 0) {
    return false
  }

  const tokenPrefix = line
    .slice(tokenStart, start + (text.startsWith('/') ? 1 : 0))
    .replace(/^[`"'([{]+/, '')
  return URI_SCHEME_REGEX.test(tokenPrefix)
}

export interface FilePathMatch {
  text: string
  start: number
}

/** Finds file-like path tokens while excluding URL host/path fragments. */
export function findFilePathMatches(line: string): FilePathMatch[] {
  if (!line.includes('/') && !line.includes('\\') && !line.includes(':')) {
    return []
  }

  const matches: FilePathMatch[] = []
  const quotedRanges: Array<[number, number]> = []
  for (const match of line.matchAll(/(?<![\p{L}\p{N}_])(["'`])([^\r\n]*?)\1(?::\d+(?::\d+)?)?/gu)) {
    const start = match.index ?? 0
    quotedRanges.push([start, start + match[0].length])
    if (looksLikeFilePath(match[2]) && !isUrlAdjacentPathMatch(line, start, match[0])) {
      // Outside-quote suffixes need the wrapper to keep the link contiguous.
      const outsideSuffix = /["'`]:\d/.test(match[0])
      matches.push({
        text: outsideSuffix ? match[0] : match[2],
        start: outsideSuffix ? start : start + 1
      })
    }
  }

  for (const match of line.matchAll(FILE_PATH_LINK_REGEX)) {
    if (
      quotedRanges.some(([start, end]) => (match.index ?? 0) >= start && (match.index ?? 0) < end)
    ) {
      continue
    }
    let text = trimTrailingPathPunctuation(match[0])
    let start = match.index ?? -1
    const wrapper = WRAPPER_PAIRS.find(
      ([opening, closing]) => text.startsWith(opening) && line[start + text.length] === closing
    )
    if (wrapper) {
      text = text.slice(wrapper[0].length)
      start += wrapper[0].length
    }

    if (
      !text ||
      !looksLikeFilePath(text) ||
      start < 0 ||
      isUrlAdjacentPathMatch(line, start, text)
    ) {
      continue
    }

    matches.push({ text, start })
  }

  return matches.sort((a, b) => a.start - b.start)
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
  return findFilePathMatches(line).map(({ text, start }) => ({
    range: {
      start: { x: start + 1, y: lineNumber },
      end: { x: start + text.length, y: lineNumber }
    },
    text,
    activate: onActivate
  }))
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

function buildResolutionCandidates(candidate: string, roots: string[]): string[] {
  if (isAbsolutePath(candidate)) {
    const absoluteCandidate = canonicalizeAbsolutePath(candidate)
    if (!absoluteCandidate) {
      return []
    }

    return roots.some((root) => isPathWithinRoot(absoluteCandidate, root))
      ? [absoluteCandidate]
      : []
  }

  const paths: string[] = []

  for (const root of roots) {
    const resolvedCandidate = canonicalizeAbsolutePath(joinPath(root, candidate))
    if (!resolvedCandidate || !isPathWithinRoot(resolvedCandidate, root)) {
      continue
    }

    if (!paths.includes(resolvedCandidate)) {
      paths.push(resolvedCandidate)
    }
  }

  return paths
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

/** Removes trailing :line or :line:column suffixes from a path token. */
export function stripLineColumnSuffix(value: string): string {
  return value.replace(/:(\d+)(?::\d+)?$/, '')
}

function parseLineColumnSuffix(value: string): { line?: number; column?: number } {
  const match = value.match(/:(\d+)(?::(\d+))?$/)
  if (!match) {
    return {}
  }

  const line = Number.parseInt(match[1], 10)
  const parsedColumn = match[2] ? Number.parseInt(match[2], 10) : null

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
  return trimWrappedPath(stripLineColumnSuffix(trimWrappedPath(value)))
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

/** Resolves a terminal path token to an existing file using terminal and project context. */
export async function resolveFilePathCandidate(
  rawCandidate: string,
  context: FilePathResolutionContext
): Promise<FilePathResolutionResult> {
  const candidate = extractPathCandidate(rawCandidate)

  if (candidate.endsWith('/') || candidate.endsWith('\\')) {
    return { ok: false, reason: 'not-file' }
  }

  const allowedRoots = getAllowedRoots(context)

  if (allowedRoots.length === 0) {
    return { ok: false, reason: 'missing-context' }
  }

  const absoluteCandidates = buildResolutionCandidates(candidate, allowedRoots)

  if (absoluteCandidates.length === 0) {
    return { ok: false, reason: 'not-found' }
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

    useWorkspaceStore.getState().addEditorTab(resolution.path)
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
