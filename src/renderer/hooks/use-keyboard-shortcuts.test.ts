import { renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
import { KEYBINDING_SCHEME_KEY, KEYBOARD_SHORTCUTS_KEY } from '@/types/settings'
import { useKeyboardShortcutsLoader } from './use-keyboard-shortcuts'

const mocks = vi.hoisted(() => ({
  read: vi.fn(),
  loadFiles: vi.fn(),
  isTauri: vi.fn(() => true)
}))

vi.mock('@/lib/api', () => ({
  persistenceApi: { read: mocks.read, write: vi.fn(), writeDebounced: vi.fn() }
}))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: mocks.isTauri }))
vi.mock('@/lib/keybinding-schemes', async () => {
  const actual = await vi.importActual<typeof import('@/lib/keybinding-schemes')>(
    '@/lib/keybinding-schemes'
  )
  return { ...actual, loadKeybindingSchemeFiles: mocks.loadFiles }
})

function persisted(values: Record<string, unknown>) {
  mocks.read.mockImplementation(async (key: string) => ({
    success: true,
    data: values[key] ?? null
  }))
}

describe('useKeyboardShortcutsLoader', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.isTauri.mockReturnValue(true)
    const store = useKeyboardShortcutsStore.getState()
    store.setUserSchemes([], [])
    store.applyScheme('se-default')
    store.resetAllShortcuts()
    mocks.loadFiles.mockResolvedValue({ success: true, data: { dir: '/k', files: [] } })
  })

  it('restores the saved scheme on top of the saved custom keys', async () => {
    persisted({
      [KEYBOARD_SHORTCUTS_KEY]: { sidebarToggle: { customKey: 'cmd+shift+s' } },
      [KEYBINDING_SCHEME_KEY]: 'iterm2'
    })
    renderHook(() => useKeyboardShortcutsLoader())

    await waitFor(() => expect(useKeyboardShortcutsStore.getState().schemeId).toBe('iterm2'))
    const { shortcuts } = useKeyboardShortcutsStore.getState()
    expect(shortcuts.sidebarToggle.customKey).toBe('cmd+shift+s')
  })

  it('can restore a user scheme because the files load first', async () => {
    mocks.loadFiles.mockResolvedValue({
      success: true,
      data: {
        dir: '/k',
        files: [{ name: 'mine.json', content: '{"bindings":{"splitRight":"cmd+e"}}' }]
      }
    })
    persisted({ [KEYBINDING_SCHEME_KEY]: 'user:mine' })
    renderHook(() => useKeyboardShortcutsLoader())

    await waitFor(() => expect(useKeyboardShortcutsStore.getState().schemeId).toBe('user:mine'))
    expect(useKeyboardShortcutsStore.getState().shortcuts.splitRight.defaultKey).toBe('cmd+e')
  })

  it('reports an unreadable scheme folder', async () => {
    mocks.loadFiles.mockResolvedValue({ success: false, error: 'EACCES', code: 'X' })
    persisted({})
    renderHook(() => useKeyboardShortcutsLoader())

    await waitFor(() =>
      expect(useKeyboardShortcutsStore.getState().schemeIssues).toEqual([
        { file: '', message: 'EACCES' }
      ])
    )
  })

  it('does not touch scheme files in the web client', async () => {
    mocks.isTauri.mockReturnValue(false)
    persisted({})
    renderHook(() => useKeyboardShortcutsLoader())

    // The scheme files would be requested in the same tick as the reads.
    await waitFor(() => expect(mocks.read).toHaveBeenCalledTimes(2))
    expect(mocks.loadFiles).not.toHaveBeenCalled()
  })
})
