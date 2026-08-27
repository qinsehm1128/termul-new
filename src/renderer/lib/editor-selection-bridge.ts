/**
 * A seam for reading and editing a *virtualized* editor's selection.
 *
 * CodeMirror keeps only the lines near the viewport in the DOM. Anything that
 * reads a selection through `window.getSelection()` or moves it with
 * `document.execCommand('selectAll')` therefore sees the rendered slice and
 * nothing else — Select All followed by Copy yields the document from its start
 * down to roughly the bottom of the window, which is what a user reports as
 * "copying only part of the file".
 *
 * The fix is to ask the editor for its own selection instead of the DOM. That
 * cannot be a direct import: the global context menu that needs this lives in
 * the main bundle, while the editor is loaded lazily, so importing CodeMirror
 * here would pull the whole editor into the initial download. The editor
 * registers an adapter when its chunk loads, and callers degrade to the DOM
 * path when nothing has registered.
 */

export interface EditorSelectionAdapter {
  /**
   * Selected text as the editor's own state knows it.
   *
   * `null` means "not an element this adapter owns" — distinct from `''`, an
   * element it owns with an empty selection. Callers must not collapse the two:
   * `null` has to fall through to the DOM path, `''` must not.
   */
  selectedText: (element: HTMLElement) => string | null
  /** Select the whole document. `false` when the element is not owned. */
  selectAll: (element: HTMLElement) => boolean
  /** Delete the current selection. `false` when not owned or nothing selected. */
  deleteSelection: (element: HTMLElement) => boolean
}

let adapter: EditorSelectionAdapter | null = null

/** Install the adapter. Pass `null` to uninstall (tests). */
export function registerEditorSelectionAdapter(next: EditorSelectionAdapter | null): void {
  adapter = next
}

export function getEditorSelectionAdapter(): EditorSelectionAdapter | null {
  return adapter
}
