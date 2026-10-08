import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockIsTauri, mockClone, mockRead, mockWrite } = vi.hoisted(() => ({
  mockIsTauri: vi.fn(() => true),
  mockClone: vi.fn(),
  mockRead: vi.fn(),
  mockWrite: vi.fn()
}))

vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: mockIsTauri }))
vi.mock('@/lib/api', () => ({
  dialogApi: { selectDirectory: vi.fn() },
  filesystemApi: {
    readDirectory: vi.fn().mockResolvedValue({ success: true, data: [] }),
    createDirectory: vi.fn()
  },
  gitApi: { clone: mockClone, init: vi.fn() },
  shellApi: {
    getAvailableShells: vi.fn().mockResolvedValue({
      success: true,
      data: { default: { name: 'zsh' }, available: [{ name: 'zsh', displayName: 'Zsh' }] }
    })
  },
  persistenceApi: { read: mockRead, write: mockWrite }
}))
vi.mock('sonner', () => ({ toast: { promise: vi.fn() } }))
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
        'zsh'
      )
    )
    expect(mockClone).toHaveBeenCalledWith(
      'https://github.com/qinsehm1128/prefect_test.git',
      '/Users/qs/project/ns/python',
      'prefect_test'
    )
    await waitFor(() =>
      expect(mockWrite).toHaveBeenCalledWith('projects/clone-parent-dirs', [
        '/Users/qs/project/ns/python'
      ])
    )
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
