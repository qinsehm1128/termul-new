import type { EditorWorkspaceCandidate } from '@shared/types/editor-workspace.types'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useProjectStore } from '@/stores/project-store'
import { ImportEditorWorkspacesDialog } from './ImportEditorWorkspacesDialog'

const { mockList, mockParseFile, mockToast } = vi.hoisted(() => ({
  mockList: vi.fn(),
  mockParseFile: vi.fn(),
  mockToast: vi.fn()
}))

vi.mock('@/lib/editor-workspace-api', () => ({
  editorWorkspaceApi: {
    list: mockList,
    parseFile: mockParseFile
  }
}))

vi.mock('@/hooks/use-toast', () => ({
  toast: mockToast
}))

vi.mock('@/lib/log-api', () => ({
  logFrontendError: vi.fn()
}))

function candidate(
  partial: Partial<EditorWorkspaceCandidate> &
    Pick<EditorWorkspaceCandidate, 'id' | 'path' | 'name'>
): EditorWorkspaceCandidate {
  return {
    editor: 'vscode',
    source: 'recent',
    ...partial
  }
}

describe('ImportEditorWorkspacesDialog', () => {
  beforeEach(() => {
    mockList.mockReset()
    mockParseFile.mockReset()
    mockToast.mockReset()
    mockList.mockResolvedValue({ success: true, data: { candidates: [] } })
    mockParseFile.mockResolvedValue({ success: true, data: { candidates: [] } })
    useProjectStore.setState({
      projects: [],
      groups: [],
      activeProjectId: '',
      activeGroupId: null,
      isLoaded: true
    })
  })

  it('imports selected recents through addProject and skips already-imported paths', async () => {
    useProjectStore.setState({
      projects: [
        {
          id: 'existing',
          name: 'Alpha',
          color: 'blue',
          path: '/Users/me/alpha/',
          gitBranch: 'main'
        }
      ],
      activeProjectId: 'existing'
    })
    mockList.mockResolvedValue({
      success: true,
      data: {
        candidates: [
          candidate({
            id: 'vscode:/users/me/alpha',
            name: 'alpha',
            path: '/Users/me/alpha'
          }),
          candidate({
            id: 'cursor:/users/me/beta',
            editor: 'cursor',
            name: 'beta',
            path: '/Users/me/beta'
          })
        ]
      }
    })

    render(<ImportEditorWorkspacesDialog isOpen onClose={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByText('alpha')).toBeInTheDocument()
      expect(screen.getByText('Already in Termul')).toBeInTheDocument()
    })

    fireEvent.click(screen.getByTestId('editor-import-confirm'))

    await waitFor(() => {
      const paths = useProjectStore.getState().projects.map((project) => project.path)
      expect(paths).toContain('/Users/me/alpha/')
      expect(paths).toContain('/Users/me/beta')
      expect(paths.filter((path) => path?.toLowerCase().includes('alpha'))).toHaveLength(1)
    })
  })

  it('groups multi-root .code-workspace folders and calls addProject for each', async () => {
    mockParseFile.mockResolvedValue({
      success: true,
      data: {
        candidates: [
          candidate({
            id: 'vscode:/tmp/one',
            name: 'one',
            path: '/tmp/one',
            source: 'workspace-file'
          }),
          candidate({
            id: 'vscode:/tmp/two',
            name: 'two',
            path: '/tmp/two',
            source: 'workspace-file'
          })
        ]
      }
    })

    render(<ImportEditorWorkspacesDialog isOpen onClose={vi.fn()} />)

    await waitFor(() => {
      expect(mockList).toHaveBeenCalled()
    })

    fireEvent.change(screen.getByTestId('editor-import-workspace-path'), {
      target: { value: '/tmp/app.code-workspace' }
    })
    fireEvent.click(screen.getByText('Parse'))

    await waitFor(() => {
      expect(screen.getByText('one')).toBeInTheDocument()
      expect(screen.getByText('two')).toBeInTheDocument()
    })

    fireEvent.click(screen.getByTestId('editor-import-confirm'))

    await waitFor(() => {
      const { projects, groups } = useProjectStore.getState()
      expect(projects.map((project) => project.path)).toEqual(
        expect.arrayContaining(['/tmp/one', '/tmp/two'])
      )
      expect(groups).toHaveLength(1)
      expect(groups[0]?.name).toBe('app')
      expect(groups[0]?.projectIds).toHaveLength(2)
    })
  })

  it('shows a failure state when web parse has no path (browse unavailable)', async () => {
    render(<ImportEditorWorkspacesDialog isOpen onClose={vi.fn()} />)

    await waitFor(() => {
      expect(mockList).toHaveBeenCalled()
    })

    fireEvent.click(screen.getByText('Parse'))

    await waitFor(() => {
      expect(mockParseFile).not.toHaveBeenCalled()
      expect(mockToast).toHaveBeenCalledWith(
        expect.objectContaining({
          description: 'Choose or paste a .code-workspace file.'
        })
      )
    })
  })
})
