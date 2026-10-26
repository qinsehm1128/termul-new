import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useWorkspaceActiveTabSync } from './use-workspace-active-tab-sync'

const state = vi.hoisted(() => {
  const listeners = new Set<(next: typeof pane, previous: typeof pane) => void>()
  const pane = {
    root: {},
    activePaneId: 'pane-1',
    file: '/project/main.ts' as string | null,
    terminal: null as string | null
  }
  return {
    listeners,
    pane,
    editor: { activeFilePath: null as string | null, setActiveFilePath: vi.fn() },
    terminals: { activeTerminalId: null as string | null, selectTerminal: vi.fn() }
  }
})
vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: {
    getState: () => state.pane,
    subscribe: (listener: (next: typeof state.pane, previous: typeof state.pane) => void) => {
      state.listeners.add(listener)
      return () => state.listeners.delete(listener)
    }
  },
  getActiveFilePathFromTree: (pane: typeof state.pane) => pane.file,
  getActiveTerminalIdFromTree: (pane: typeof state.pane) => pane.terminal
}))
vi.mock('@/stores/editor-store', () => ({ useEditorStore: { getState: () => state.editor } }))
vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: { getState: () => state.terminals }
}))

function changePane(file: string | null, terminal: string | null = null): void {
  const previous = { ...state.pane }
  Object.assign(state.pane, { root: {}, file, terminal })
  for (const listener of state.listeners) listener(state.pane, previous)
}

beforeEach(() => {
  state.listeners.clear()
  Object.assign(state.pane, {
    root: {},
    activePaneId: 'pane-1',
    file: '/project/main.ts',
    terminal: null
  })
  state.editor.activeFilePath = null
  state.terminals.activeTerminalId = null
  vi.clearAllMocks()
  state.editor.setActiveFilePath.mockImplementation((path: string | null) => {
    state.editor.activeFilePath = path
  })
  state.terminals.selectTerminal.mockImplementation((id: string) => {
    state.terminals.activeTerminalId = id
  })
})

describe('project active tab ownership', () => {
  it('mirrors the current project immediately and follows its pane changes', () => {
    renderHook(() => useWorkspaceActiveTabSync(true))
    expect(state.editor.activeFilePath).toBe('/project/main.ts')
    act(() => changePane(null, 'project-terminal'))
    expect(state.editor.activeFilePath).toBeNull()
    expect(state.terminals.activeTerminalId).toBe('project-terminal')
  })

  it('never overwrites quick workspace state while project panes are hidden', () => {
    state.editor.activeFilePath = '/quick/readme.md'
    state.terminals.activeTerminalId = 'quick-terminal'
    renderHook(() => useWorkspaceActiveTabSync(false))
    act(() => changePane('/project/changed.ts', 'project-terminal'))
    expect(state.editor.activeFilePath).toBe('/quick/readme.md')
    expect(state.terminals.activeTerminalId).toBe('quick-terminal')
    expect(state.editor.setActiveFilePath).not.toHaveBeenCalled()
    expect(state.terminals.selectTerminal).not.toHaveBeenCalled()
    expect(state.listeners.size).toBe(0)
  })

  it('releases ownership on quick navigation and restores it on project return', () => {
    const { rerender, unmount } = renderHook(({ enabled }) => useWorkspaceActiveTabSync(enabled), {
      initialProps: { enabled: true }
    })
    rerender({ enabled: false })
    state.editor.activeFilePath = '/quick/file.ts'
    act(() => changePane('/project/restored.ts'))
    expect(state.editor.activeFilePath).toBe('/quick/file.ts')
    rerender({ enabled: true })
    expect(state.editor.activeFilePath).toBe('/project/restored.ts')
    expect(state.listeners.size).toBe(1)
    unmount()
    expect(state.listeners.size).toBe(0)
  })
})
