import type { DirectoryEntry } from '@shared/types/filesystem.types'
import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { activeFileDirectoryChain, useRevealActiveFile } from './use-reveal-active-file'

type RevealOptions = Parameters<typeof useRevealActiveFile>[0]

const entry = (path: string, type: DirectoryEntry['type']): DirectoryEntry => ({
  name: path.split('/').pop() ?? path,
  path,
  type,
  extension: type === 'file' ? 'ts' : null,
  size: 0,
  modifiedAt: 0
})

const options = (overrides: Partial<RevealOptions> = {}): RevealOptions => ({
  activeFilePath: '/project/src/app.ts',
  rootPaths: ['/project'],
  expandedDirs: new Set(['/project', '/project/src']),
  directoryContents: new Map([
    ['/project', [entry('/project/src', 'directory')]],
    ['/project/src', [entry('/project/src/app.ts', 'file')]]
  ]),
  loadingDirs: new Set(),
  enabled: true,
  toggleDirectory: vi.fn().mockResolvedValue(undefined),
  revealPath: vi.fn(() => 42),
  ...overrides
})

describe('activeFileDirectoryChain', () => {
  it('returns only displayed ancestors and normalizes Windows separators', () => {
    expect(activeFileDirectoryChain('C:\\work\\src\\app.ts', ['C:/work'])).toEqual([
      'C:/work',
      'C:/work/src'
    ])
  })

  it('uses the longest matching root across multiple roots', () => {
    expect(
      activeFileDirectoryChain('/workspace/packages/app/src/main.ts', [
        '/workspace',
        '/workspace/packages/app'
      ])
    ).toEqual(['/workspace/packages/app', '/workspace/packages/app/src'])
  })

  it('rejects path-prefix collisions and files equal to a root', () => {
    expect(activeFileDirectoryChain('/project-other/src/app.ts', ['/project'])).toBeNull()
    expect(activeFileDirectoryChain('/project', ['/project'])).toBeNull()
  })
})

describe('useRevealActiveFile', () => {
  beforeEach(() => {
    vi.stubGlobal(
      'requestAnimationFrame',
      vi.fn(() => 42)
    )
    vi.stubGlobal('cancelAnimationFrame', vi.fn())
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('reveals only after every ancestor is loaded and the file is present', () => {
    const toggleDirectory = vi.fn().mockResolvedValue(undefined)
    const revealPath = vi.fn(() => 42)
    const initial = options({
      expandedDirs: new Set(['/project']),
      directoryContents: new Map([
        ['/project', [entry('/project/src', 'directory')]],
        ['/project/src', [entry('/project/src/app.ts', 'file')]]
      ]),
      toggleDirectory,
      revealPath
    })
    const { rerender } = renderHook((props: RevealOptions) => useRevealActiveFile(props), {
      initialProps: initial
    })

    expect(toggleDirectory).toHaveBeenCalledTimes(1)
    expect(toggleDirectory).toHaveBeenCalledWith('/project/src')
    expect(revealPath).not.toHaveBeenCalled()

    rerender({ ...initial, expandedDirs: new Set(['/project', '/project/src']) })

    expect(revealPath).toHaveBeenCalledTimes(1)
    expect(revealPath).toHaveBeenCalledWith('/project/src/app.ts')
  })

  it('waits for a loading ancestor and never toggles it closed', () => {
    const toggleDirectory = vi.fn().mockResolvedValue(undefined)
    const initial = options({
      expandedDirs: new Set(['/project']),
      directoryContents: new Map([['/project', [entry('/project/src', 'directory')]]]),
      loadingDirs: new Set(['/project/src']),
      toggleDirectory
    })
    const { rerender } = renderHook((props: RevealOptions) => useRevealActiveFile(props), {
      initialProps: initial
    })

    expect(toggleDirectory).not.toHaveBeenCalled()

    rerender({ ...initial, loadingDirs: new Set(), expandedDirs: new Set(['/project']) })

    expect(toggleDirectory).toHaveBeenCalledTimes(1)
    expect(toggleDirectory).toHaveBeenCalledWith('/project/src')
  })

  it('does not open another root when the active file is outside all roots', () => {
    const toggleDirectory = vi.fn().mockResolvedValue(undefined)
    const revealPath = vi.fn(() => 42)
    renderHook(() =>
      useRevealActiveFile(
        options({
          activeFilePath: '/outside/app.ts',
          rootPaths: ['/project', '/workspace'],
          toggleDirectory,
          revealPath
        })
      )
    )

    expect(toggleDirectory).not.toHaveBeenCalled()
    expect(revealPath).not.toHaveBeenCalled()
  })

  it('supports a secondary root and only reveals a file actually listed by its parent', () => {
    const toggleDirectory = vi.fn().mockResolvedValue(undefined)
    const revealPath = vi.fn(() => 42)
    const initial = options({
      activeFilePath: '/workspace/src/main.ts',
      rootPaths: ['/project', '/workspace'],
      expandedDirs: new Set(['/workspace']),
      directoryContents: new Map([
        ['/workspace', [entry('/workspace/src', 'directory')]],
        ['/workspace/src', [entry('/workspace/src/other.ts', 'file')]]
      ]),
      toggleDirectory,
      revealPath
    })
    const { rerender } = renderHook((props: RevealOptions) => useRevealActiveFile(props), {
      initialProps: initial
    })

    expect(toggleDirectory).toHaveBeenCalledWith('/workspace/src')
    rerender({ ...initial, expandedDirs: new Set(['/workspace', '/workspace/src']) })

    expect(revealPath).not.toHaveBeenCalled()
  })

  it('cancels a stale request when the active file changes rapidly', async () => {
    let resolveOld: (() => void) | undefined
    const oldToggle = new Promise<void>((resolve) => {
      resolveOld = resolve
    })
    const toggleDirectory = vi.fn().mockReturnValueOnce(oldToggle).mockResolvedValue(undefined)
    const initial = options({
      expandedDirs: new Set(['/project']),
      directoryContents: new Map([
        ['/project', [entry('/project/src', 'directory'), entry('/project/new', 'directory')]]
      ]),
      toggleDirectory
    })
    const { rerender } = renderHook((props: RevealOptions) => useRevealActiveFile(props), {
      initialProps: initial
    })

    expect(toggleDirectory).toHaveBeenCalledWith('/project/src')
    rerender({
      ...initial,
      activeFilePath: '/project/new/current.ts',
      directoryContents: new Map([
        ['/project', [entry('/project/src', 'directory'), entry('/project/new', 'directory')]]
      ])
    })
    await act(async () => resolveOld?.())

    expect(toggleDirectory).toHaveBeenCalledWith('/project/new')
    expect(toggleDirectory).not.toHaveBeenCalledWith('/project/src', expect.anything())
  })

  it('cancels a scheduled reveal when the hook becomes disabled', () => {
    const revealPath = vi.fn(() => 99)
    const initial = options({ revealPath })
    const { rerender } = renderHook((props: RevealOptions) => useRevealActiveFile(props), {
      initialProps: initial
    })

    expect(revealPath).toHaveBeenCalledWith('/project/src/app.ts')
    rerender({ ...initial, enabled: false })

    expect(cancelAnimationFrame).toHaveBeenCalledWith(99)
  })
})
