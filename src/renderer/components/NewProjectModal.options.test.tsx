import type { DirectoryEntry, IpcResult } from '@shared/types/ipc.types'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const {
  mockIsTauri,
  mockCreateDirectory,
  mockCreateFile,
  mockReadDirectory,
  mockInit,
  mockAddIgnoreRules,
  mockScaffoldProject,
  mockGetAvailableShells,
  mockToastPromise
} = vi.hoisted(() => ({
  mockIsTauri: vi.fn(() => true),
  mockCreateDirectory: vi.fn(),
  mockCreateFile: vi.fn(),
  mockReadDirectory: vi.fn(),
  mockInit: vi.fn(),
  mockAddIgnoreRules: vi.fn(),
  mockScaffoldProject: vi.fn(),
  mockGetAvailableShells: vi.fn(),
  mockToastPromise: vi.fn()
}))

vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: mockIsTauri }))
vi.mock('@/lib/api', () => ({
  dialogApi: { selectDirectory: vi.fn() },
  filesystemApi: {
    createDirectory: mockCreateDirectory,
    createFile: mockCreateFile,
    readDirectory: mockReadDirectory
  },
  gitApi: { init: mockInit, addIgnoreRules: mockAddIgnoreRules },
  shellApi: { getAvailableShells: mockGetAvailableShells },
  persistenceApi: { read: vi.fn(), write: vi.fn() }
}))
vi.mock('@/lib/project-templates', () => ({
  BUILT_IN_TEMPLATES: [
    {
      id: 'node',
      name: 'Node',
      description: 'Node project',
      files: [{ path: 'README.md', content: '# {{projectName}}' }]
    },
    { id: 'empty', name: 'Empty', description: 'Empty project' }
  ],
  scaffoldProject: mockScaffoldProject
}))
vi.mock('@/stores/app-settings-store', () => ({ useDefaultProjectColor: () => 'blue' }))
vi.mock('sonner', () => ({
  toast: {
    promise: mockToastPromise,
    error: vi.fn(),
    success: vi.fn(),
    loading: vi.fn()
  }
}))

import { NewProjectModal } from './NewProjectModal'

const createButton = () => screen.getByRole('button', { name: 'Create' })
const engineSelect = () => screen.getByLabelText('Terminal screen observation')
const ignoreTextarea = () => screen.getByLabelText('Git ignore rules')

async function renderCreateModal(
  props: Partial<React.ComponentProps<typeof NewProjectModal>> = {}
) {
  const onCreateProject = props.onCreateProject ?? vi.fn()
  render(<NewProjectModal isOpen onClose={vi.fn()} onCreateProject={onCreateProject} {...props} />)
  await screen.findByRole('option', { name: 'Bash' })
  await new Promise((resolve) => setTimeout(resolve, 0))
  fireEvent.change(screen.getByPlaceholderText('My Project'), { target: { value: 'Regression' } })
  fireEvent.change(screen.getByPlaceholderText('No directory selected'), {
    target: { value: '/tmp/regression' }
  })
  await waitFor(() => expect(mockReadDirectory).toHaveBeenCalledWith('/tmp/regression'))
  await screen.findByLabelText('Initialize Git repository in this directory')
  return onCreateProject
}

async function submitCreate() {
  fireEvent.click(createButton())
  await waitFor(() => expect(mockToastPromise).toHaveBeenCalled())
}

describe('NewProjectModal terminal engine and editable gitignore', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockIsTauri.mockReturnValue(true)
    mockGetAvailableShells.mockResolvedValue({
      success: true,
      data: { default: { name: 'bash' }, available: [{ name: 'bash', displayName: 'Bash' }] }
    })
    mockReadDirectory.mockResolvedValue({ success: true, data: [] })
    mockCreateDirectory.mockResolvedValue({ success: true })
    mockCreateFile.mockResolvedValue({ success: true })
    mockInit.mockResolvedValue({ success: true })
    mockAddIgnoreRules.mockResolvedValue(undefined)
    mockScaffoldProject.mockResolvedValue({ success: true, data: undefined })
    mockToastPromise.mockImplementation(
      (promise: Promise<unknown>, options: Record<string, (value?: unknown) => void>) => {
        promise.then(options.success, options.error)
      }
    )
  })

  it('forwards selected Ghostty engine as the sixth create callback argument', async () => {
    const onCreateProject = await renderCreateModal()
    fireEvent.change(engineSelect(), { target: { value: 'ghostty-vt' } })
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))
    await submitCreate()

    await waitFor(() =>
      expect(onCreateProject).toHaveBeenCalledWith(
        'Regression',
        'blue',
        '/tmp/regression',
        'bash',
        undefined,
        'ghostty-vt'
      )
    )
  })

  it('uses existing engine by default and resets engine and ignore rules when reopened', async () => {
    const onClose = vi.fn()
    const { rerender } = render(
      <NewProjectModal isOpen onClose={onClose} onCreateProject={vi.fn()} />
    )
    await screen.findByRole('option', { name: 'Bash' })
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(engineSelect()).toHaveValue('existing')
    fireEvent.change(engineSelect(), { target: { value: 'ghostty-vt' } })
    fireEvent.change(screen.getByPlaceholderText('My Project'), { target: { value: 'Changed' } })
    fireEvent.change(screen.getByPlaceholderText('No directory selected'), {
      target: { value: '/tmp/x' }
    })
    await screen.findByLabelText('Initialize Git repository in this directory')
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))
    fireEvent.change(ignoreTextarea(), { target: { value: 'custom\nrule\n' } })

    rerender(<NewProjectModal isOpen={false} onClose={onClose} onCreateProject={vi.fn()} />)
    rerender(<NewProjectModal isOpen onClose={onClose} onCreateProject={vi.fn()} />)
    await screen.findByRole('option', { name: 'Bash' })
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(engineSelect()).toHaveValue('existing')
    fireEvent.change(screen.getByPlaceholderText('No directory selected'), {
      target: { value: '/tmp/reopened' }
    })
    await waitFor(() => expect(mockReadDirectory).toHaveBeenCalledWith('/tmp/reopened'))
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))
    expect(ignoreTextarea()).toHaveValue('.qin-code/\n.se-manage/\n.se-manager/\n')
  })

  it('renders the browser engine preference as read-only', async () => {
    mockIsTauri.mockReturnValue(false)
    render(<NewProjectModal isOpen onClose={vi.fn()} onCreateProject={vi.fn()} />)
    await screen.findByRole('option', { name: 'Bash' })
    expect(engineSelect()).toBeDisabled()
    expect(screen.getByText(/read-only on this client/i)).toBeInTheDocument()
  })

  it.each([
    ['default multiline rules', '.qin-code/\n.se-manage/\n.se-manager/\n'],
    ['custom multiline rules', 'node_modules/\n.cache/\n*.local\n'],
    ['empty rules', '']
  ])('submits %s to git ignore writer', async (_name, content) => {
    const onCreateProject = await renderCreateModal()
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))
    fireEvent.change(ignoreTextarea(), { target: { value: content } })
    await submitCreate()

    if (content.trim()) {
      await waitFor(() =>
        expect(mockAddIgnoreRules).toHaveBeenCalledWith('/tmp/regression', content)
      )
    } else {
      expect(mockAddIgnoreRules).not.toHaveBeenCalled()
    }
    await waitFor(() => expect(onCreateProject).toHaveBeenCalledTimes(1))
  })

  it('does not write custom ignore rules when git initialization is unchecked', async () => {
    await renderCreateModal()
    await submitCreate()

    expect(mockInit).not.toHaveBeenCalled()
    expect(mockAddIgnoreRules).not.toHaveBeenCalled()
  })

  it('completes template scaffolding before writing custom ignore rules', async () => {
    const order: string[] = []
    mockScaffoldProject.mockImplementation(async () => {
      order.push('scaffold')
      return { success: true, data: undefined }
    })
    mockAddIgnoreRules.mockImplementation(async () => {
      order.push('ignore')
    })
    await renderCreateModal()
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))
    fireEvent.change(ignoreTextarea(), { target: { value: 'custom\n' } })
    fireEvent.change(screen.getAllByRole('combobox')[0], { target: { value: 'empty' } })
    await submitCreate()

    await waitFor(() => expect(mockAddIgnoreRules).toHaveBeenCalled())
    expect(order.indexOf('scaffold')).toBeLessThan(order.indexOf('ignore'))
  })

  it('does not report creation success when initialization fails', async () => {
    const onCreateProject = await renderCreateModal()
    mockInit.mockRejectedValueOnce(new Error('init failed'))
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))
    await submitCreate()

    await waitFor(() => expect(mockInit).toHaveBeenCalled())
    expect(onCreateProject).not.toHaveBeenCalled()
    expect(mockAddIgnoreRules).not.toHaveBeenCalled()
  })

  it('does not report creation success when scaffolding fails', async () => {
    const onCreateProject = await renderCreateModal()
    mockScaffoldProject.mockResolvedValueOnce({ success: false, error: 'scaffold failed' })
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))
    await submitCreate()

    await waitFor(() => expect(mockScaffoldProject).toHaveBeenCalled())
    expect(onCreateProject).not.toHaveBeenCalled()
    expect(mockAddIgnoreRules).not.toHaveBeenCalled()
  })

  it('does not report creation success when custom ignore writing fails', async () => {
    const onCreateProject = await renderCreateModal()
    mockAddIgnoreRules.mockRejectedValueOnce(new Error('ignore failed'))
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))
    fireEvent.change(ignoreTextarea(), { target: { value: 'custom\n' } })
    await submitCreate()

    await waitFor(() => expect(mockAddIgnoreRules).toHaveBeenCalled())
    expect(onCreateProject).not.toHaveBeenCalled()
  })

  it('preserves entered fields when delayed shell detection resolves', async () => {
    let resolveShells!: (value: {
      success: true
      data: { default: { name: string }; available: { name: string; displayName: string }[] }
    }) => void
    mockGetAvailableShells.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveShells = resolve
      })
    )

    render(<NewProjectModal isOpen onClose={vi.fn()} onCreateProject={vi.fn()} />)
    fireEvent.change(screen.getByPlaceholderText('My Project'), {
      target: { value: 'Typed Before Detection' }
    })
    fireEvent.change(screen.getByPlaceholderText('No directory selected'), {
      target: { value: '/tmp/typed-before-detection' }
    })
    await waitFor(() =>
      expect(mockReadDirectory).toHaveBeenCalledWith('/tmp/typed-before-detection')
    )
    await screen.findByLabelText('Initialize Git repository in this directory')
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))
    fireEvent.change(engineSelect(), { target: { value: 'ghostty-vt' } })
    fireEvent.change(ignoreTextarea(), { target: { value: 'custom-before-shells\n' } })

    resolveShells({
      success: true,
      data: { default: { name: 'bash' }, available: [{ name: 'bash', displayName: 'Bash' }] }
    })

    await screen.findByRole('option', { name: 'Bash' })
    expect(screen.getByPlaceholderText('My Project')).toHaveValue('Typed Before Detection')
    expect(screen.getByPlaceholderText('No directory selected')).toHaveValue(
      '/tmp/typed-before-detection'
    )
    expect(engineSelect()).toHaveValue('ghostty-vt')
    expect(ignoreTextarea()).toHaveValue('custom-before-shells\n')
  })

  it('ignores stale slow folder results and clears init state for a newer non-empty path', async () => {
    const pending = new Map<string, (value: IpcResult<DirectoryEntry[]>) => void>()
    mockReadDirectory.mockImplementation(
      (path: string) =>
        new Promise((resolve) => {
          pending.set(path, resolve)
        })
    )

    render(<NewProjectModal isOpen onClose={vi.fn()} onCreateProject={vi.fn()} />)
    await screen.findByRole('option', { name: 'Bash' })

    fireEvent.change(screen.getByPlaceholderText('No directory selected'), {
      target: { value: '/tmp/setup' }
    })
    await waitFor(() => expect(pending.has('/tmp/setup')).toBe(true))
    await act(async () => {
      pending.get('/tmp/setup')?.({ success: true, data: [] })
    })
    await screen.findByLabelText('Initialize Git repository in this directory')
    fireEvent.click(screen.getByLabelText('Initialize Git repository in this directory'))

    fireEvent.change(screen.getByPlaceholderText('No directory selected'), {
      target: { value: '/tmp/A' }
    })
    fireEvent.change(screen.getByPlaceholderText('No directory selected'), {
      target: { value: '/tmp/B' }
    })
    await waitFor(() => {
      expect(pending.has('/tmp/A')).toBe(true)
      expect(pending.has('/tmp/B')).toBe(true)
      expect(
        screen.queryByLabelText('Initialize Git repository in this directory')
      ).not.toBeInTheDocument()
      expect(screen.queryByLabelText('Git ignore rules')).not.toBeInTheDocument()
    })
    await act(async () => {
      pending.get('/tmp/B')?.({
        success: true,
        data: [
          {
            name: 'existing.txt',
            path: '/tmp/B/existing.txt',
            type: 'file',
            extension: 'txt',
            size: 0,
            modifiedAt: 0
          }
        ]
      })
    })
    await waitFor(() =>
      expect(
        screen.queryByLabelText('Initialize Git repository in this directory')
      ).not.toBeInTheDocument()
    )
    expect(screen.queryByLabelText('Git ignore rules')).not.toBeInTheDocument()

    await act(async () => {
      pending.get('/tmp/A')?.({ success: true, data: [] })
    })
    expect(
      screen.queryByLabelText('Initialize Git repository in this directory')
    ).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Git ignore rules')).not.toBeInTheDocument()
  })

  it('does not perform filesystem or git writes for editor import', async () => {
    const onImportFromEditor = vi.fn()
    render(
      <NewProjectModal
        isOpen
        onClose={vi.fn()}
        onCreateProject={vi.fn()}
        onImportFromEditor={onImportFromEditor}
      />
    )
    await screen.findByRole('option', { name: 'Bash' })
    fireEvent.click(screen.getByTestId('new-project-import-editors'))

    expect(onImportFromEditor).toHaveBeenCalledTimes(1)
    expect(mockCreateDirectory).not.toHaveBeenCalled()
    expect(mockCreateFile).not.toHaveBeenCalled()
    expect(mockInit).not.toHaveBeenCalled()
    expect(mockAddIgnoreRules).not.toHaveBeenCalled()
  })
})
