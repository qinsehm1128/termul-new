/**
 * The virtualized-editor path through copy / cut / select-all.
 *
 * CodeMirror renders only the lines near the viewport, so the DOM-based
 * implementations of these actions saw the rendered slice and nothing else —
 * Select All then Copy produced the file from its start down to the bottom of
 * the window. These pin the adapter path that replaced them.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  type EditorSelectionAdapter,
  registerEditorSelectionAdapter
} from '../editor-selection-bridge'
import { copySelection, cutSelection, selectAllFocused } from '../text-edit-ops'

const copied: string[] = []

vi.mock('@/lib/copy-text', () => ({
  copyText: vi.fn(async (text: string) => {
    copied.push(text)
    return true
  })
}))

vi.mock('@/lib/api', () => ({ clipboardApi: { readText: vi.fn() } }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))

/** Stands in for `.cm-content`: contentEditable, and owned by the adapter. */
function mountEditorElement(): HTMLElement {
  const el = document.createElement('div')
  el.contentEditable = 'true'
  // jsdom does not derive isContentEditable from the attribute.
  Object.defineProperty(el, 'isContentEditable', { value: true, configurable: true })
  document.body.append(el)
  el.focus()
  Object.defineProperty(document, 'activeElement', { value: el, configurable: true })
  return el
}

function adapterFor(el: HTMLElement, overrides: Partial<EditorSelectionAdapter> = {}) {
  const owns = (candidate: HTMLElement): boolean => candidate === el
  return {
    selectedText: (candidate: HTMLElement) => (owns(candidate) ? 'WHOLE DOCUMENT' : null),
    selectAll: vi.fn((candidate: HTMLElement) => owns(candidate)),
    deleteSelection: vi.fn((candidate: HTMLElement) => owns(candidate)),
    ...overrides
  }
}

describe('text-edit-ops with a virtualized editor', () => {
  beforeEach(() => {
    copied.length = 0
    document.body.innerHTML = ''
  })

  afterEach(() => {
    registerEditorSelectionAdapter(null)
    vi.restoreAllMocks()
  })

  it("copies the editor's own selection, not what the DOM has rendered", async () => {
    const el = mountEditorElement()
    // What a DOM read would have returned: the rendered slice.
    vi.spyOn(window, 'getSelection').mockReturnValue({
      toString: () => 'ONLY THE VISIBLE LINES',
      rangeCount: 0
    } as unknown as Selection)
    registerEditorSelectionAdapter(adapterFor(el))

    await copySelection()

    expect(copied).toEqual(['WHOLE DOCUMENT'])
  })

  it('selects the whole document instead of the rendered lines', () => {
    const el = mountEditorElement()
    const adapter = adapterFor(el)
    registerEditorSelectionAdapter(adapter)
    const execCommand = vi.fn(() => true)
    document.execCommand = execCommand

    expect(selectAllFocused()).toBe(true)

    expect(adapter.selectAll).toHaveBeenCalledWith(el)
    expect(execCommand).not.toHaveBeenCalled()
  })

  it('deletes through the editor so cut removes what it copied', async () => {
    const el = mountEditorElement()
    const adapter = adapterFor(el)
    registerEditorSelectionAdapter(adapter)
    const execCommand = vi.fn(() => true)
    document.execCommand = execCommand

    expect(await cutSelection()).toBe(true)

    expect(copied).toEqual(['WHOLE DOCUMENT'])
    expect(adapter.deleteSelection).toHaveBeenCalledWith(el)
    expect(execCommand).not.toHaveBeenCalled()
  })

  it('leaves elements the adapter does not own on the DOM path', async () => {
    // Ordinary page text and plain contentEditables must keep working.
    mountEditorElement()
    vi.spyOn(window, 'getSelection').mockReturnValue({
      toString: () => 'PLAIN DOM SELECTION',
      rangeCount: 0
    } as unknown as Selection)
    registerEditorSelectionAdapter({
      selectedText: () => null,
      selectAll: () => false,
      deleteSelection: () => false
    })

    await copySelection()

    expect(copied).toEqual(['PLAIN DOM SELECTION'])
  })

  it('treats an owned element with an empty selection as empty, not as unowned', async () => {
    // `''` and `null` mean different things: collapsing them would fall through
    // to the DOM and copy the rendered lines for an editor with no selection.
    mountEditorElement()
    vi.spyOn(window, 'getSelection').mockReturnValue({
      toString: () => 'ONLY THE VISIBLE LINES',
      rangeCount: 0
    } as unknown as Selection)
    registerEditorSelectionAdapter({
      selectedText: () => '',
      selectAll: () => true,
      deleteSelection: () => true
    })

    expect(await copySelection()).toBe(false)
    expect(copied).toEqual([])
  })

  it('still uses the DOM when no editor has registered', async () => {
    mountEditorElement()
    vi.spyOn(window, 'getSelection').mockReturnValue({
      toString: () => 'PLAIN DOM SELECTION',
      rangeCount: 0
    } as unknown as Selection)

    await copySelection()

    expect(copied).toEqual(['PLAIN DOM SELECTION'])
  })
})
