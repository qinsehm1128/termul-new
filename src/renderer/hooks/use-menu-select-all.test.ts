/**
 * The native Edit ▸ Select All item is a custom menu item now, so the whole
 * feature hangs on this listener: if it does not run `selectAllFocused()`,
 * Cmd+A does nothing at all (the OS item that used to handle it is gone).
 *
 * The editor-aware part is what the change exists for — `selectAllFocused()`
 * asks CodeMirror for its whole document, while the removed native selector
 * only reached the rendered DOM.
 */

import { renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const listeners = new Map<string, () => void>()
const unlisten = vi.fn()

const mockListen = vi.fn((event: string, handler: () => void) => {
  listeners.set(event, handler)
  return Promise.resolve(unlisten)
})

const selectAllFocused = vi.fn<() => boolean>()

vi.mock('@/lib/tauri-event', () => ({
  listen: (event: string, handler: () => void) => mockListen(event, handler)
}))

vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => true,
  cleanupTauriListener: (pending?: Promise<() => void>) => {
    void pending?.then((fn) => {
      fn()
    })
  }
}))

vi.mock('@/lib/text-edit-ops', () => ({
  selectAllFocused: () => selectAllFocused()
}))

import { useMenuSelectAll } from './use-menu-select-all'

function fireSelectAll(): void {
  const handler = listeners.get('menu:select-all')
  if (!handler) throw new Error('no listener registered for menu:select-all')
  handler()
}

describe('useMenuSelectAll', () => {
  let execCommand: ReturnType<typeof vi.fn>

  beforeEach(() => {
    listeners.clear()
    mockListen.mockClear()
    selectAllFocused.mockReset()
    execCommand = vi.fn(() => true)
    document.execCommand = execCommand as unknown as typeof document.execCommand
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('subscribes to the topic the Rust menu handler emits', () => {
    selectAllFocused.mockReturnValue(true)
    renderHook(() => {
      useMenuSelectAll()
    })

    expect(mockListen).toHaveBeenCalledTimes(1)
    expect(mockListen.mock.calls[0]?.[0]).toBe('menu:select-all')
  })

  it('routes the menu event through the editor-aware selectAllFocused', () => {
    selectAllFocused.mockReturnValue(true)
    renderHook(() => {
      useMenuSelectAll()
    })

    fireSelectAll()

    expect(selectAllFocused).toHaveBeenCalledTimes(1)
    // The editor handled it — no document-wide selection on top of it, which
    // would extend the selection over the surrounding UI chrome.
    expect(execCommand).not.toHaveBeenCalled()
  })

  it('falls back to a document-wide selection when nothing editable is focused', () => {
    selectAllFocused.mockReturnValue(false)
    renderHook(() => {
      useMenuSelectAll()
    })

    fireSelectAll()

    expect(execCommand).toHaveBeenCalledWith('selectAll')
  })
})
