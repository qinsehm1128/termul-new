import { create } from 'zustand'
import {
  EDITOR_REVEAL_LINE_EVENT,
  PENDING_REVEAL_LINE_GLOBAL,
  type PendingRevealLineWindow
} from '@/lib/editor-events'
import { releaseShownFile } from '@/lib/file-open-target'
import { useEditorStore } from '@/stores/editor-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'

/**
 * The floating document preview shows files in three shapes. A collapsed ball
 * survives everything else so the preview can be parked without losing the
 * open documents; the window and full-screen shapes share the same content.
 */
export type FloatingDocMode = 'collapsed' | 'floating' | 'fullscreen'

export interface FloatingDocPoint {
  x: number
  y: number
}

export interface FloatingDocRect {
  x: number
  y: number
  width: number
  height: number
}

export interface OpenDocumentOptions {
  line?: number
  column?: number
  /** `fullscreen` promotes the preview; omitted keeps the current shape. */
  mode?: 'floating' | 'fullscreen'
}

/** Ball diameter in px. Kept here so clamping and the component agree. */
export const FLOATING_BALL_SIZE = 44
/** Gap the ball keeps from every viewport edge. */
export const FLOATING_BALL_MARGIN = 8
/** Default floating card size; the component shrinks it on small viewports. */
export const FLOATING_DOC_DEFAULT_WIDTH = 760
export const FLOATING_DOC_DEFAULT_HEIGHT = 600

const BALL_POSITION_STORAGE_KEY = 'floating-doc-preview/ball-position'

interface ViewportSize {
  width: number
  height: number
}

function currentViewport(): ViewportSize {
  if (typeof window === 'undefined') return { width: 0, height: 0 }
  return { width: window.innerWidth, height: window.innerHeight }
}

/**
 * Keep the ball fully visible. A stored position from a larger window (or
 * another monitor) would otherwise land off-screen and become unreachable.
 */
export function clampBallPosition(
  position: FloatingDocPoint,
  viewport: ViewportSize = currentViewport(),
  size: number = FLOATING_BALL_SIZE,
  margin: number = FLOATING_BALL_MARGIN
): FloatingDocPoint {
  const maxX = Math.max(margin, viewport.width - size - margin)
  const maxY = Math.max(margin, viewport.height - size - margin)
  return {
    x: Math.min(maxX, Math.max(margin, position.x)),
    y: Math.min(maxY, Math.max(margin, position.y))
  }
}

function readStoredBallPosition(): FloatingDocPoint | null {
  if (typeof window === 'undefined') return null
  try {
    const raw = window.localStorage.getItem(BALL_POSITION_STORAGE_KEY)
    if (!raw) return null
    const parsed: unknown = JSON.parse(raw)
    if (
      !parsed ||
      typeof parsed !== 'object' ||
      typeof (parsed as FloatingDocPoint).x !== 'number' ||
      typeof (parsed as FloatingDocPoint).y !== 'number'
    ) {
      return null
    }
    return clampBallPosition(parsed as FloatingDocPoint)
  } catch {
    return null
  }
}

function writeStoredBallPosition(position: FloatingDocPoint): void {
  if (typeof window === 'undefined') return
  try {
    window.localStorage.setItem(BALL_POSITION_STORAGE_KEY, JSON.stringify(position))
  } catch {
    // A full or disabled storage must not break dragging.
  }
}

function defaultRect(): FloatingDocRect {
  const viewport = currentViewport()
  const width = Math.min(FLOATING_DOC_DEFAULT_WIDTH, Math.max(320, viewport.width - 32))
  const height = Math.min(FLOATING_DOC_DEFAULT_HEIGHT, Math.max(280, viewport.height - 32))
  return {
    x: Math.max(FLOATING_BALL_MARGIN, Math.round((viewport.width - width) / 2)),
    y: Math.max(FLOATING_BALL_MARGIN, Math.round((viewport.height - height) / 2)),
    width,
    height
  }
}

function nextActivePath(docPaths: string[], removed: string, activePath: string | null): string {
  if (activePath && activePath !== removed) return activePath
  const index = docPaths.indexOf(removed)
  const neighbour = docPaths[index + 1] ?? docPaths[index - 1]
  return neighbour ?? docPaths.find((path) => path !== removed) ?? ''
}

export interface FloatingDocState {
  docPaths: string[]
  activePath: string | null
  mode: FloatingDocMode
  ballPosition: FloatingDocPoint | null
  rect: FloatingDocRect

  /**
   * Open `path` in the editor store and show it here. Rejects when the file
   * cannot be read (missing, binary, too large) so callers can report it.
   */
  openDocument: (path: string, options?: OpenDocumentOptions) => Promise<void>
  setActiveDoc: (path: string) => void
  /** Drop a document from the preview; `discard` cancels auto-save and drops unshared dirty buffers. */
  closeDocument: (path: string, options?: { discard?: boolean }) => void
  pruneMissingDocuments: () => void
  closeAllDocuments: () => void
  setMode: (mode: FloatingDocMode) => void
  toggleCollapse: () => void
  toggleFullscreen: () => void
  setBallPosition: (position: FloatingDocPoint) => void
  setRect: (rect: FloatingDocRect) => void
  /** Move a preview document into the workspace tab strip instead. */
  moveToWorkspaceTab: (path: string) => void
}

export const useFloatingDocStore = create<FloatingDocState>((set, get) => ({
  docPaths: [],
  activePath: null,
  mode: 'floating',
  ballPosition: readStoredBallPosition(),
  rect: defaultRect(),

  openDocument: async (path, options = {}): Promise<void> => {
    await useEditorStore.getState().openFile(path)

    // The tab limit can silently refuse a new file; showing an empty preview
    // for it would hang on the loading state forever.
    if (!useEditorStore.getState().openFiles.has(path)) {
      throw new Error(`Unable to open ${path}`)
    }

    if (options.line && options.line > 0) {
      const lineNumber = options.line
      const columnNumber = Math.max(1, options.column ?? 1)
      useEditorStore.getState().updateCursorPosition(path, lineNumber, columnNumber)
      // Rendered views have no lines to reveal; only source editors scroll.
      if (useEditorStore.getState().openFiles.get(path)?.viewMode !== 'code') {
        useEditorStore.getState().setViewMode(path, 'code')
      }
      const detail = { filePath: path, lineNumber }
      ;(window as unknown as PendingRevealLineWindow)[PENDING_REVEAL_LINE_GLOBAL] = detail
      window.dispatchEvent(new CustomEvent(EDITOR_REVEAL_LINE_EVENT, { detail }))
    }

    set((state) => ({
      docPaths: state.docPaths.includes(path) ? state.docPaths : [...state.docPaths, path],
      activePath: path,
      mode: options.mode ?? (state.mode === 'fullscreen' ? 'fullscreen' : 'floating')
    }))
  },

  setActiveDoc: (path): void => {
    if (!get().docPaths.includes(path)) return
    set({ activePath: path })
  },

  closeDocument: (path, options = {}): void => {
    const { docPaths, activePath, mode } = get()
    if (!docPaths.includes(path)) return
    const remaining = docPaths.filter((candidate) => candidate !== path)
    set({
      docPaths: remaining,
      activePath: remaining.length === 0 ? null : nextActivePath(docPaths, path, activePath),
      mode: remaining.length === 0 ? 'floating' : mode
    })
    if (options.discard) {
      const inPanes = getAllLeafPanes(useWorkspaceStore.getState().root).some((leaf) =>
        leaf.tabs.some((tab) => tab.type === 'editor' && tab.filePath === path)
      )
      if (!inPanes) {
        useEditorStore.getState().closeFile(path)
        return
      }
    }
    // Persisted open files outlive the preview unless a pane still shows them.
    releaseShownFile(path)
  },

  pruneMissingDocuments: (): void => {
    const { docPaths, activePath, mode } = get()
    if (docPaths.length === 0) return
    const { openFiles } = useEditorStore.getState()
    const remaining = docPaths.filter((path) => openFiles.has(path))
    if (remaining.length === docPaths.length) return
    const nextActive =
      activePath && remaining.includes(activePath) ? activePath : (remaining[0] ?? null)
    set({
      docPaths: remaining,
      activePath: nextActive,
      mode: remaining.length === 0 ? 'floating' : mode
    })
  },

  closeAllDocuments: (): void => {
    const { docPaths } = get()
    set({ docPaths: [], activePath: null, mode: 'floating' })
    docPaths.forEach(releaseShownFile)
  },

  setMode: (mode): void => {
    if (get().docPaths.length === 0) return
    set({ mode })
  },

  toggleCollapse: (): void => {
    const { mode, docPaths } = get()
    if (docPaths.length === 0) return
    set({ mode: mode === 'collapsed' ? 'floating' : 'collapsed' })
  },

  toggleFullscreen: (): void => {
    const { mode, docPaths } = get()
    if (docPaths.length === 0) return
    set({ mode: mode === 'fullscreen' ? 'floating' : 'fullscreen' })
  },

  setBallPosition: (position): void => {
    const clamped = clampBallPosition(position)
    set({ ballPosition: clamped })
    writeStoredBallPosition(clamped)
  },

  setRect: (rect): void => {
    set({ rect })
  },

  moveToWorkspaceTab: (path): void => {
    const { docPaths, activePath } = get()
    useWorkspaceStore.getState().addEditorTab(path)
    const remaining = docPaths.filter((candidate) => candidate !== path)
    set({
      docPaths: remaining,
      activePath: remaining.length === 0 ? null : nextActivePath(docPaths, path, activePath),
      mode: remaining.length === 0 ? 'floating' : get().mode
    })
  }
}))

/** Reset helper for tests: no documents, defaults, no stored position. */
export function resetFloatingDocStoreForTests(): void {
  if (typeof window !== 'undefined') {
    try {
      window.localStorage.removeItem(BALL_POSITION_STORAGE_KEY)
    } catch {
      // Ignore storage errors in tests.
    }
  }
  useFloatingDocStore.setState({
    docPaths: [],
    activePath: null,
    mode: 'floating',
    ballPosition: null,
    rect: defaultRect()
  })
}
