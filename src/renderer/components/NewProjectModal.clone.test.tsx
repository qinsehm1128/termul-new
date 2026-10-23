import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const {
  mockIsTauri,
  mockClone,
  mockCancelClone,
  mockRead,
  mockWrite,
  mockCreateDirectory,
  mockCreateFile,
  mockInit,
  mockAddIgnoreRules
} = vi.hoisted(() => ({
  mockIsTauri: vi.fn(() => true),
  mockClone: vi.fn(),
  mockCancelClone: vi.fn(),
  mockRead: vi.fn(),
  mockWrite: vi.fn(),
  mockCreateDirectory: vi.fn(),
  mockCreateFile: vi.fn(),
  mockInit: vi.fn(),
  mockAddIgnoreRules: vi.fn()
}))

vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: mockIsTauri }))
vi.mock('@/lib/api', () => ({
  dialogApi: { selectDirectory: vi.fn() },
  filesystemApi: {
    readDirectory: vi.fn().mockResolvedValue({ success: true, data: [] }),
    createDirectory: mockCreateDirectory,
    createFile: mockCreateFile
  },
  gitApi: {
    clone: mockClone,
    cancelClone: mockCancelClone,
    init: mockInit,
    addIgnoreRules: mockAddIgnoreRules
  },
  shellApi: {
    getAvailableShells: vi.fn().mockResolvedValue({
      success: true,
      data: { default: { name: 'zsh' }, available: [{ name: 'zsh', displayName: 'Zsh' }] }
    })
  },
  persistenceApi: { read: mockRead, write: mockWrite }
}))
vi.mock('sonner', () => ({
  toast: Object.assign(vi.fn(), {
    promise: vi.fn(),
    custom: vi.fn(),
    dismiss: vi.fn(),
    success: vi.fn(),
    error: vi.fn()
  })
}))
vi.mock('@/stores/app-settings-store', () => ({ useDefaultProjectColor: () => 'blue' }))

import { NewProjectModal } from './NewProjectModal'

async function renderModal(onCreateProject = vi.fn()) {
  render(<NewProjectModal isOpen onClose={vi.fn()} onCreateProject={onCreateProject} />)
  // The form resets once the shell list arrives; type after that.
  await screen.findByRole('option', { name: 'Zsh' })
  return onCreateProject
}

const repoInput = () => screen.getByLabelText('Clone from Git (optional)')
const createButton = () => screen.getByRole('button', { name: 'Create' })

describe('NewProjectModal clone from Git', () => {
  beforeEach(() => {
    mockIsTauri.mockReturnValue(true)
    mockClone.mockReset()
    mockRead.mockReset().mockResolvedValue({ success: true, data: ['/Users/qs/project/ns/python'] })
    mockWrite.mockReset().mockResolvedValue({ success: true })
    mockCreateDirectory.mockReset().mockResolvedValue({ success: true })
    mockCreateFile.mockReset().mockResolvedValue({ success: true })
    mockInit.mockReset().mockResolvedValue({ success: true })
    mockAddIgnoreRules.mockReset().mockResolvedValue(undefined)
  })

  it('names the project after the repository and clones into the chosen folder', async () => {
    mockClone.mockResolvedValue('/Users/qs/project/ns/python/prefect_test')
    const onCreateProject = await renderModal()

    fireEvent.change(repoInput(), {
      target: { value: 'https://github.com/qinsehm1128/prefect_test.git' }
    })
    expect(screen.getByPlaceholderText('My Project')).toHaveValue('prefect_test')

    fireEvent.click(await screen.findByRole('button', { name: '/Users/qs/project/ns/python' }))
    expect(
      screen.getByText('Will be cloned to /Users/qs/project/ns/python/prefect_test')
    ).toBeInTheDocument()

    fireEvent.click(createButton())

    await waitFor(() =>
      expect(onCreateProject).toHaveBeenCalledWith(
        'prefect_test',
        'blue',
        '/Users/qs/project/ns/python/prefect_test',
        'zsh',
        undefined,
        'existing'
      )
    )
    expect(mockClone).toHaveBeenCalledWith(
      expect.any(String),
      'https://github.com/qinsehm1128/prefect_test.git',
      '/Users/qs/project/ns/python',
      'prefect_test',
      expect.any(Function)
    )
    await waitFor(() =>
      expect(mockWrite).toHaveBeenCalledWith('projects/clone-parent-dirs', [
        '/Users/qs/project/ns/python'
      ])
    )
    expect(mockAddIgnoreRules).not.toHaveBeenCalled()
  })

  it('forwards selected Ghostty engine and detected shell without env vars when cloning', async () => {
    mockClone.mockResolvedValue('/Users/qs/project/ns/python/prefect_test')
    const onCreateProject = await renderModal()

    fireEvent.change(screen.getByLabelText('Clone from Git (optional)'), {
      target: { value: 'https://github.com/qinsehm1128/prefect_test.git' }
    })
    fireEvent.change(screen.getByLabelText(/Terminal screen observation/i), {
      target: { value: 'ghostty-vt' }
    })
    fireEvent.click(await screen.findByRole('button', { name: '/Users/qs/project/ns/python' }))
    fireEvent.click(createButton())

    await waitFor(() =>
      expect(onCreateProject).toHaveBeenCalledWith(
        'prefect_test',
        'blue',
        '/Users/qs/project/ns/python/prefect_test',
        'zsh',
        undefined,
        'ghostty-vt'
      )
    )
    expect(mockAddIgnoreRules).not.toHaveBeenCalled()
  })
  it('does not create a project for a cancelled clone', async () => {
    mockClone.mockRejectedValue('GIT_CLONE_CANCELLED')
    const onCreateProject = await renderModal()
    fireEvent.change(repoInput(), {
      target: { value: 'https://github.com/qinsehm1128/prefect_test.git' }
    })
    fireEvent.click(await screen.findByRole('button', { name: '/Users/qs/project/ns/python' }))
    fireEvent.click(createButton())

    await waitFor(() => expect(mockClone).toHaveBeenCalled())
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(onCreateProject).not.toHaveBeenCalled()
    expect(mockWrite).not.toHaveBeenCalled()
  })

  it('keeps a name the user typed', async () => {
    await renderModal()
    fireEvent.change(screen.getByPlaceholderText('My Project'), {
      target: { value: 'Prefect' }
    })
    fireEvent.change(repoInput(), { target: { value: 'qinsehm1128/prefect_test' } })
    expect(screen.getByPlaceholderText('My Project')).toHaveValue('Prefect')
  })

  it('locks the template to an empty project while cloning', async () => {
    await renderModal()
    const template = screen.getAllByRole('combobox')[0]
    expect(template).not.toBeDisabled()
    fireEvent.change(repoInput(), { target: { value: 'qinsehm1128/prefect_test' } })
    expect(template).toBeDisabled()
    expect(template).toHaveValue('empty')
  })

  it('will not create from an address it cannot clone', async () => {
    await renderModal()
    fireEvent.change(repoInput(), { target: { value: 'not a repo' } })
    fireEvent.change(screen.getByPlaceholderText('My Project'), {
      target: { value: 'x' }
    })
    fireEvent.change(screen.getByPlaceholderText('No directory selected'), {
      target: { value: '/tmp' }
    })
    expect(screen.getByText('Not a repository address that can be cloned')).toBeInTheDocument()
    expect(createButton()).toBeDisabled()
  })

  it('offers cloning only on the desktop', async () => {
    mockIsTauri.mockReturnValue(false)
    await renderModal()
    expect(screen.queryByLabelText('Clone from Git (optional)')).not.toBeInTheDocument()
  })
})
