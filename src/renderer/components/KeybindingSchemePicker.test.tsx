import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
import { KEYBINDING_SCHEME_KEY } from '@/types/settings'
import { KeybindingSchemePicker } from './KeybindingSchemePicker'

const mocks = vi.hoisted(() => ({
  write: vi.fn().mockResolvedValue({ success: true, data: undefined }),
  openWithExternalApp: vi.fn().mockResolvedValue({ success: true, data: undefined }),
  loadFiles: vi.fn(),
  isTauri: vi.fn(() => true)
}))

vi.mock('@/lib/api', () => ({
  persistenceApi: {
    read: vi.fn().mockResolvedValue({ success: true, data: null }),
    write: mocks.write,
    writeDebounced: vi.fn()
  },
  openerApi: { openWithExternalApp: mocks.openWithExternalApp }
}))

vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: mocks.isTauri }))

vi.mock('@/lib/keybinding-schemes', async () => {
  const actual = await vi.importActual<typeof import('@/lib/keybinding-schemes')>(
    '@/lib/keybinding-schemes'
  )
  return { ...actual, loadKeybindingSchemeFiles: mocks.loadFiles }
})

describe('KeybindingSchemePicker', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.isTauri.mockReturnValue(true)
    const store = useKeyboardShortcutsStore.getState()
    store.setUserSchemes([], [])
    store.applyScheme('se-default')
    store.resetAllShortcuts()
  })

  it('applies and persists the chosen scheme', async () => {
    render(<KeybindingSchemePicker />)
    fireEvent.change(screen.getByLabelText('Keybinding scheme'), { target: { value: 'iterm2' } })

    await waitFor(() => expect(mocks.write).toHaveBeenCalledWith(KEYBINDING_SCHEME_KEY, 'iterm2'))
    expect(useKeyboardShortcutsStore.getState().schemeId).toBe('iterm2')
  })

  it('reloads user scheme files, lists them, and reports bad entries', async () => {
    mocks.loadFiles.mockResolvedValue({
      success: true,
      data: {
        dir: '/home/me/.se-manager/keybindings',
        files: [{ name: 'work.json', content: '{"name":"Work","bindings":{"splitRigth":"cmd+e"}}' }]
      }
    })
    render(<KeybindingSchemePicker />)
    fireEvent.click(screen.getByRole('button', { name: 'Reload' }))

    expect(await screen.findByRole('option', { name: 'Work (custom)' })).toBeInTheDocument()
    expect(screen.getByText('work.json: unknown action "splitRigth"')).toBeInTheDocument()
    expect(mocks.loadFiles).toHaveBeenCalledWith(false)
  })

  it('creates and opens the scheme folder', async () => {
    mocks.loadFiles.mockResolvedValue({
      success: true,
      data: { dir: '/home/me/.se-manager/keybindings', files: [] }
    })
    render(<KeybindingSchemePicker />)
    fireEvent.click(screen.getByRole('button', { name: 'Open folder' }))

    await waitFor(() =>
      expect(mocks.openWithExternalApp).toHaveBeenCalledWith('/home/me/.se-manager/keybindings')
    )
    expect(mocks.loadFiles).toHaveBeenCalledWith(true)
  })

  it('hides the file actions in the web client', () => {
    mocks.isTauri.mockReturnValue(false)
    render(<KeybindingSchemePicker />)
    expect(screen.queryByRole('button', { name: 'Reload' })).toBeNull()
    expect(screen.getByRole('option', { name: 'Ghostty' })).toBeInTheDocument()
  })
})
