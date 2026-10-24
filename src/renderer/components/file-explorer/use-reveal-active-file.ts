import type { DirectoryEntry } from '@shared/types/filesystem.types'
import { useEffect, useRef } from 'react'
import { logFrontendError } from '@/lib/log-api'

interface RevealOptions {
  activeFilePath: string | null
  rootPaths: string[]
  /** Store root-set identity changes even when a project reopens at the same path. */
  rootScope?: object
  expandedDirs: Set<string>
  directoryContents: Map<string, DirectoryEntry[]>
  loadingDirs: Set<string>
  enabled: boolean
  toggleDirectory: (path: string) => Promise<void>
  revealPath: (path: string) => number
}

interface RevealRequest {
  path: string
  rootsKey: string
  rootScope?: object
  directories: string[]
  attempted: Set<string>
}

const normalize = (path: string): string => path.replace(/\\/g, '/')

/** Only descend into a displayed root, including secondary roots and drive roots. */
export function activeFileDirectoryChain(path: string, roots: string[]): string[] | null {
  const normalized = normalize(path)
  const root = roots
    .map(normalize)
    .sort((a, b) => b.length - a.length)
    .find((candidate) =>
      normalized.startsWith(candidate.endsWith('/') ? candidate : `${candidate}/`)
    )
  if (!root) return null
  const prefix = root.endsWith('/') ? root : `${root}/`
  const directories = [root]
  let current = prefix.slice(0, -1)
  for (const segment of normalized.slice(prefix.length).split('/').slice(0, -1)) {
    current += `/${segment}`
    directories.push(current)
  }
  return directories
}

/**
 * Follow the editor, not tree selection. Advance one loaded ancestor at a time:
 * this joins in-flight root loads without toggling a directory closed, and a
 * new file/root/visibility request cancels the previous pending reveal.
 */
export function useRevealActiveFile(options: RevealOptions): void {
  const {
    activeFilePath,
    rootPaths,
    rootScope,
    expandedDirs,
    directoryContents,
    loadingDirs,
    enabled,
    toggleDirectory,
    revealPath
  } = options
  const path = activeFilePath ? normalize(activeFilePath) : null
  const rootsKey = JSON.stringify(rootPaths.map(normalize))
  const pending = useRef<RevealRequest | null>(null)
  const frame = useRef<number | null>(null)

  useEffect(() => {
    const directories = path ? activeFileDirectoryChain(path, JSON.parse(rootsKey)) : null
    pending.current =
      enabled && path && directories
        ? { path, rootsKey, rootScope, directories, attempted: new Set() }
        : null
    return () => {
      pending.current = null
      if (frame.current !== null) window.cancelAnimationFrame(frame.current)
      frame.current = null
    }
  }, [path, rootsKey, rootScope, enabled])

  useEffect(() => {
    const request = pending.current
    if (
      !enabled ||
      !request ||
      request.path !== path ||
      request.rootsKey !== rootsKey ||
      request.rootScope !== rootScope
    )
      return
    const fail = (code: string): void => {
      if (pending.current !== request) return
      pending.current = null
      void logFrontendError({
        level: 'warn',
        source: 'FileExplorer:autoReveal',
        message: `operation=active_file_reveal stable_code=${code}`
      })
    }
    for (const [index, dir] of request.directories.entries()) {
      if (loadingDirs.has(dir)) return
      if (expandedDirs.has(dir) && directoryContents.has(dir)) continue
      if (request.attempted.has(dir) || expandedDirs.has(dir)) {
        fail('DIRECTORY_UNAVAILABLE')
        return
      }
      const parent = request.directories[index - 1]
      if (
        parent &&
        !directoryContents
          .get(parent)
          ?.some((entry) => entry.type === 'directory' && normalize(entry.path) === dir)
      ) {
        fail('DIRECTORY_NOT_FOUND')
        return
      }
      request.attempted.add(dir)
      void toggleDirectory(dir).catch(() => fail('DIRECTORY_LOAD_FAILED'))
      return
    }
    // Deleted/filtered entries are not a reason to open another root or change selection.
    const parent = request.directories.at(-1)
    if (
      parent &&
      directoryContents
        .get(parent)
        ?.some((entry) => entry.type === 'file' && normalize(entry.path) === request.path)
    ) {
      frame.current = revealPath(request.path)
    }
    pending.current = null
  }, [
    enabled,
    path,
    rootsKey,
    rootScope,
    expandedDirs,
    directoryContents,
    loadingDirs,
    toggleDirectory,
    revealPath
  ])
}
