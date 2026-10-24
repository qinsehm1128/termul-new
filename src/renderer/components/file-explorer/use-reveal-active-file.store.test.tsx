import type { DirectoryEntry } from '@shared/types/filesystem.types'
import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useFileExplorerStore } from '@/stores/file-explorer-store'
import { useRevealActiveFile } from './use-reveal-active-file'

const { readDirectory, logError } = vi.hoisted(() => ({
  readDirectory: vi.fn(),
  logError: vi.fn()
}))
vi.mock('@/lib/api', () => ({ filesystemApi: { readDirectory } }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: logError }))

type Result = { success: true; data: DirectoryEntry[] } | { success: false; error: string }
const entry = (path: string, type: DirectoryEntry['type'] = 'file'): DirectoryEntry => ({
  path,
  name: path.split('/').at(-1) ?? path,
  type,
  extension: null,
  size: 0,
  modifiedAt: 0
})
const requests = new Map<string, Array<(result: Result) => void>>()
const revealPath = vi.fn(() => 42)

function mount(active: string) {
  return renderHook(
    ({ path }) => {
      const state = useFileExplorerStore()
      useRevealActiveFile({
        activeFilePath: path,
        rootPaths: state.rootPath ? [state.rootPath] : [],
        rootScope: state.roots,
        expandedDirs: state.expandedDirs,
        loadingDirs: state.loadingDirs,
        directoryContents: state.directoryContents,
        enabled: true,
        toggleDirectory: state.toggleDirectory,
        revealPath
      })
    },
    { initialProps: { path: active } }
  )
}

async function resolve(path: string, result: Result, index = 0) {
  const finish = requests.get(path)?.[index]
  expect(finish, `directory request ${path}[${index}]`).toBeDefined()
  await act(async () => finish?.(result))
}

beforeEach(() => {
  requests.clear()
  vi.clearAllMocks()
  readDirectory.mockImplementation(
    (path: string) =>
      new Promise<Result>((finish) => {
        requests.set(path, [...(requests.get(path) ?? []), finish])
      })
  )
  useFileExplorerStore.getState().setRootPath('/project')
})

describe('active file reveal with the real explorer store', () => {
  it('restarts the reveal when the same root is reinitialized during a load', async () => {
    mount('/project/current.ts')
    act(() => useFileExplorerStore.getState().setRootPath('/project'))
    expect(requests.get('/project')).toHaveLength(2)
    await resolve('/project', { success: true, data: [entry('/project/stale.ts')] }, 0)
    expect(revealPath).not.toHaveBeenCalled()
    await resolve('/project', { success: true, data: [entry('/project/current.ts')] }, 1)
    expect(revealPath.mock.calls).toEqual([['/project/current.ts']])
    expect(logError).not.toHaveBeenCalled()
  })

  it('joins an existing directory load after switching files in that directory', async () => {
    const { rerender } = mount('/project/src/old.ts')
    await resolve('/project', { success: true, data: [entry('/project/src', 'directory')] })
    rerender({ path: '/project/src/current.ts' })
    await resolve('/project/src', {
      success: true,
      data: [entry('/project/src/old.ts'), entry('/project/src/current.ts')]
    })
    expect(requests.get('/project/src')).toHaveLength(1)
    expect(revealPath.mock.calls).toEqual([['/project/src/current.ts']])
    expect(logError).not.toHaveBeenCalled()
  })

  it('does not let an old failed load cancel a different active directory', async () => {
    const { rerender } = mount('/project/old/file.ts')
    await resolve('/project', {
      success: true,
      data: [entry('/project/old', 'directory'), entry('/project/new', 'directory')]
    })
    rerender({ path: '/project/new/current.ts' })
    await resolve('/project/old', { success: false, error: 'old request failed' })
    await resolve('/project/new', { success: true, data: [entry('/project/new/current.ts')] })
    expect(revealPath.mock.calls).toEqual([['/project/new/current.ts']])
    expect(logError).not.toHaveBeenCalled()
  })

  it('reveals the new root even when the old root load resolves first', async () => {
    const { rerender } = mount('/project/old.ts')
    act(() => useFileExplorerStore.getState().setRootPath('/other'))
    rerender({ path: '/other/current.ts' })
    await resolve('/project', { success: true, data: [entry('/project/old.ts')] })
    expect(revealPath).not.toHaveBeenCalled()
    await resolve('/other', { success: true, data: [entry('/other/current.ts')] })
    expect(revealPath.mock.calls).toEqual([['/other/current.ts']])
    expect(logError).not.toHaveBeenCalled()
  })

  it('keeps the latest load alive when switching away and back to the same root', async () => {
    const { rerender } = mount('/project/old.ts')
    act(() => useFileExplorerStore.getState().setRootPath('/other'))
    rerender({ path: '/other/other.ts' })
    act(() => useFileExplorerStore.getState().setRootPath('/project'))
    rerender({ path: '/project/current.ts' })
    expect(requests.get('/project')).toHaveLength(2)
    await resolve('/project', { success: false, error: 'superseded root load failed' }, 0)
    await resolve('/project', { success: true, data: [entry('/project/current.ts')] }, 1)
    expect(revealPath.mock.calls).toEqual([['/project/current.ts']])
    expect(logError).not.toHaveBeenCalled()
    await resolve('/other', { success: true, data: [entry('/other/other.ts')] })
  })
})
