import { toast } from 'sonner'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { type EditorFileState, useEditorStore } from '@/stores/editor-store'
import { resetFloatingDocStoreForTests, useFloatingDocStore } from '@/stores/floating-doc-store'
import { useProjectStore } from '@/stores/project-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'
import type { Project } from '@/types/project'
import { findOwningProject, routeExternalFileOpen } from './external-file-router'

vi.mock('sonner', () => ({
  toast: { error: vi.fn(), warning: vi.fn(), success: vi.fn() }
}))

function project(id: string, path: string, overrides: Partial<Project> = {}): Project {
  return { id, name: id, color: 'blue', path, ...overrides }
}

function fileState(filePath: string): EditorFileState {
  return {
    filePath,
    content: '',
    originalContent: '',
    isDirty: false,
    language: '',
    lastModified: 1,
    viewMode: 'preview',
    cursorPosition: { line: 1, col: 1 },
    scrollTop: 0,
    operationStatus: 'idle'
  }
}

const originalOpenFile = useEditorStore.getState().openFile
const originalFloatingOpen = useFloatingDocStore.getState().openDocument

function editorTabPaths(): string[] {
  return getAllLeafPanes(useWorkspaceStore.getState().root).flatMap((leaf) =>
    leaf.tabs.flatMap((tab) => (tab.type === 'editor' ? [tab.filePath] : []))
  )
}

beforeEach(() => {
  resetFloatingDocStoreForTests()
  useEditorStore.setState({ openFiles: new Map(), activeFilePath: null })
  useEditorStore.setState({
    openFile: async (path: string) => {
      useEditorStore.setState((state) => {
        const openFiles = new Map(state.openFiles)
        openFiles.set(path, fileState(path))
        return { openFiles, activeFilePath: path }
      })
    }
  })
  useFloatingDocStore.setState({
    openDocument: vi.fn(async () => undefined)
  })
  useAppSettingsStore.setState({
    settings: {
      ...useAppSettingsStore.getState().settings,
      externalFileOpenTarget: 'floating',
      externalFileDefaultProjectId: null
    }
  })
})

afterEach(() => {
  useEditorStore.setState({ openFile: originalOpenFile })
  useFloatingDocStore.setState({ openDocument: originalFloatingOpen })
  useProjectStore.setState({ projects: [], activeProjectId: '' })
  vi.mocked(toast.error).mockClear()
})

describe('findOwningProject', () => {
  it('matches the longest path prefix and ignores archived projects', () => {
    const projects = [
      project('root', '/repo'),
      project('nested', '/repo/packages/app'),
      project('archived', '/repo/packages/app/vendor', { isArchived: true })
    ]

    expect(findOwningProject('/repo/packages/app/src/index.ts', projects)?.id).toBe('nested')
    expect(findOwningProject('/repo/other/x.ts', projects)?.id).toBe('root')
    expect(findOwningProject('/repo/packages/app/vendor/x.ts', projects)?.id).toBe('nested')
  })

  it('does not prefix-match a sibling directory', () => {
    const projects = [project('root', '/repo/app')]
    expect(findOwningProject('/repo/application/x.ts', projects)).toBeNull()
  })

  it('falls back to the configured default project', () => {
    const projects = [project('p', '/repo'), project('fallback', '/elsewhere')]
    expect(findOwningProject('/tmp/doc.md', projects, 'fallback')?.id).toBe('fallback')
    expect(findOwningProject('/tmp/doc.md', projects, 'missing')).toBeNull()
    expect(
      findOwningProject(
        '/tmp/doc.md',
        [project('archived', '/repo', { isArchived: true })],
        'archived'
      )
    ).toBeNull()
  })
})

describe('routeExternalFileOpen', () => {
  it('switches to the owning project and opens in the floating preview', async () => {
    useProjectStore.setState({
      projects: [project('a', '/repo/a'), project('b', '/repo/b')],
      activeProjectId: 'a'
    })

    const result = await routeExternalFileOpen(['/repo/b/notes.md'])

    expect(useProjectStore.getState().activeProjectId).toBe('b')
    expect(useFloatingDocStore.getState().openDocument).toHaveBeenCalledWith('/repo/b/notes.md')
    expect(result.opened).toEqual(['/repo/b/notes.md'])
    expect(result.failed).toEqual([])
  })

  it('opens a workspace tab when that is the configured target', async () => {
    useProjectStore.setState({ projects: [project('a', '/repo/a')], activeProjectId: 'a' })
    useAppSettingsStore.setState({
      settings: { ...useAppSettingsStore.getState().settings, externalFileOpenTarget: 'editor-tab' }
    })

    const result = await routeExternalFileOpen(['/repo/a/readme.md'])

    expect(editorTabPaths()).toContain('/repo/a/readme.md')
    expect(useFloatingDocStore.getState().openDocument).not.toHaveBeenCalled()
    expect(result.opened).toEqual(['/repo/a/readme.md'])
  })

  it('uses the default project when no project owns the file', async () => {
    useProjectStore.setState({
      projects: [project('a', '/repo/a'), project('default', '/repo/default')],
      activeProjectId: 'a'
    })
    useAppSettingsStore.setState({
      settings: {
        ...useAppSettingsStore.getState().settings,
        externalFileDefaultProjectId: 'default'
      }
    })

    await routeExternalFileOpen(['/tmp/report.md'])

    expect(useProjectStore.getState().activeProjectId).toBe('default')
    expect(useFloatingDocStore.getState().openDocument).toHaveBeenCalledWith('/tmp/report.md')
  })

  it('reports files that cannot be read without throwing', async () => {
    useProjectStore.setState({ projects: [project('a', '/repo/a')], activeProjectId: 'a' })
    useEditorStore.setState({
      openFile: async () => {
        throw new Error('binary file')
      }
    })
    useAppSettingsStore.setState({
      settings: { ...useAppSettingsStore.getState().settings, externalFileOpenTarget: 'editor-tab' }
    })

    const result = await routeExternalFileOpen(['/repo/a/image.bin'])

    expect(result.opened).toEqual([])
    expect(result.failed).toEqual([{ path: '/repo/a/image.bin', error: 'binary file' }])
    expect(toast.error).toHaveBeenCalledTimes(1)
  })

  it('deduplicates paths and ignores blank entries', async () => {
    useProjectStore.setState({ projects: [project('a', '/repo/a')], activeProjectId: 'a' })

    const result = await routeExternalFileOpen(['/repo/a/x.md', '/repo/a/x.md', '  '])

    expect(result.opened).toEqual(['/repo/a/x.md'])
    expect(useFloatingDocStore.getState().openDocument).toHaveBeenCalledTimes(1)
  })
})
