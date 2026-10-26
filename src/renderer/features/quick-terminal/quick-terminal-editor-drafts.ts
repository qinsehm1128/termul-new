import { useEffect } from 'react'
import { flushEditorContent } from '@/lib/editor-content-flush'
import { logFrontendError } from '@/lib/log-api'
import { persistenceApi } from '@/lib/persistence-api'
import { type EditorFileState, type EditorViewMode, useEditorStore } from '@/stores/editor-store'
import { useQuickWorkspaceStore } from './quick-terminal-workspace-store'

interface QuickEditorDraft {
  filePath: string
  content: string
  cursorPosition: { line: number; col: number }
  scrollTop: number
  viewMode: EditorViewMode
}
interface DraftState {
  paths: Set<string>
  drafts: Map<string, QuickEditorDraft>
  touched: Set<string>
  unrestored: Set<string>
  loaded: boolean
  loading?: Promise<void>
  lastQueued?: string
}
const entries = new Map<string, DraftState>()
let unsubscribeEditor: (() => void) | undefined
let unsubscribeLayouts: (() => void) | undefined
const key = (id: string): string => `quick-terminal/editor-drafts/${id}`

function warn(operation: string): void {
  // Never log paths, entry identifiers, exception text, or buffer contents.
  void logFrontendError({
    level: 'warn',
    source: 'quick-terminal-editor-drafts',
    message: `operation=${operation} code=PERSISTENCE_FAILED`
  }).catch(() => {})
}
function entry(id: string): DraftState {
  let state = entries.get(id)
  if (!state) {
    state = {
      paths: new Set(),
      drafts: new Map(),
      touched: new Set(),
      unrestored: new Set(),
      loaded: false
    }
    entries.set(id, state)
  }
  return state
}
function snapshot(file: EditorFileState): QuickEditorDraft {
  return {
    filePath: file.filePath,
    content: file.content,
    cursorPosition: file.cursorPosition,
    scrollTop: file.scrollTop,
    viewMode: file.viewMode
  }
}
function queue(id: string, state: DraftState): void {
  if (!state.loaded) return // Never replace unread drafts after a failed read.
  const data = { version: 1, files: [...state.drafts.values()] }
  const serialized = JSON.stringify(data)
  if (state.lastQueued === serialized) return
  state.lastQueued = serialized
  // An empty files array is a content-free tombstone. Use the same debounced
  // key for cleanup, avoiding delete racing an older queued dirty write.
  void persistenceApi
    .writeDebounced(key(id), data)
    .then((result) => {
      if (!result.success) {
        state.lastQueued = undefined
        warn('write')
      }
    })
    .catch(() => {
      state.lastQueued = undefined
      warn('write')
    })
}
function tabPaths(id: string): Set<string> {
  return new Set(
    (useQuickWorkspaceStore.getState().layouts[id]?.tabs ?? []).flatMap((tab) =>
      tab.type === 'editor' ? [tab.filePath] : []
    )
  )
}
function collect(): void {
  for (const [id, layout] of Object.entries(useQuickWorkspaceStore.getState().layouts)) {
    const state = entry(id)
    for (const tab of layout.tabs) if (tab.type === 'editor') state.paths.add(tab.filePath)
  }
  for (const [id, state] of entries) {
    for (const path of state.paths) {
      const file = useEditorStore.getState().openFiles.get(path)
      if (file?.isDirty) {
        state.unrestored.delete(path)
        state.touched.add(path)
        state.drafts.set(path, snapshot(file))
      } else if (
        (state.loaded || state.touched.has(path)) &&
        file &&
        !state.unrestored.has(path) &&
        file.operationStatus !== 'saving' &&
        file.operationStatus !== 'reloading'
      ) {
        state.touched.add(path)
        state.drafts.delete(path)
        // Keep ownership while the shared buffer exists: a clean late load or
        // release from a deleted layout must not become an unrelated project file.
        // Project panes explicitly sharing this path are still allowed by their filter.
      }
      // Absent buffers are NOT evidence of discard: navigation may clear the
      // shared store. Deleted/inactive layouts keep dirty ownership and drafts.
    }
    queue(id, state)
  }
}
function observe(): void {
  if (unsubscribeEditor) return
  unsubscribeEditor = useEditorStore.subscribe(collect)
  unsubscribeLayouts = useQuickWorkspaceStore.subscribe(collect)
}

/** Explicit quick ownership, including detached dirty drafts. No cwd heuristics. */
export function isQuickEditorFile(path: string): boolean {
  return (
    [...entries.values()].some((state) => state.paths.has(path)) ||
    Object.values(useQuickWorkspaceStore.getState().layouts).some((layout) =>
      layout.tabs.some((tab) => tab.type === 'editor' && tab.filePath === path)
    )
  )
}

function decode(value: unknown): QuickEditorDraft[] {
  if (value == null) return []
  if (
    typeof value !== 'object' ||
    !('version' in value) ||
    value.version !== 1 ||
    !('files' in value) ||
    !Array.isArray(value.files)
  )
    throw new Error('Invalid quick drafts')
  const result: QuickEditorDraft[] = []
  for (const file of value.files) {
    if (
      !file ||
      typeof file !== 'object' ||
      typeof file.filePath !== 'string' ||
      typeof file.content !== 'string' ||
      !file.cursorPosition ||
      !Number.isFinite(file.cursorPosition.line) ||
      !Number.isFinite(file.cursorPosition.col) ||
      !Number.isFinite(file.scrollTop) ||
      !['code', 'markdown', 'preview'].includes(file.viewMode)
    ) {
      throw new Error('Invalid quick drafts')
    }
    result.push({
      filePath: file.filePath,
      content: file.content,
      cursorPosition: { line: file.cursorPosition.line, col: file.cursorPosition.col },
      scrollTop: file.scrollTop,
      viewMode: file.viewMode
    })
  }
  return result
}
async function loadDrafts(id: string): Promise<void> {
  const state = entry(id)
  if (state.loaded) return
  if (state.loading) return state.loading
  state.loading = (async () => {
    try {
      const result = await persistenceApi.read<unknown>(key(id))
      if (!result.success && result.code !== 'KEY_NOT_FOUND') throw new Error('Draft read failed')
      const saved = decode(result.success ? result.data : null)
      for (const draft of saved) {
        state.paths.add(draft.filePath)
        if (!state.touched.has(draft.filePath)) {
          state.drafts.set(draft.filePath, draft)
          state.unrestored.add(draft.filePath)
        }
      }
      state.loaded = true
    } catch {
      warn('load')
      throw new Error('Quick editor drafts unavailable')
    } finally {
      state.loading = undefined
    }
  })()
  return state.loading
}

/** Load/restore only. The caller owns route/request-guarded activation and errors. */
export async function loadQuickEditorFile(entryId: string, path: string): Promise<void> {
  const state = entry(entryId)
  state.paths.add(path) // Claim before any asynchronous editor-store publication.
  observe()
  collect()
  await loadDrafts(entryId)
  const before = useEditorStore.getState().openFiles.get(path)
  const draft = state.drafts.get(path)
  if (draft) state.unrestored.add(path)
  await useEditorStore.getState().openFile(path, { activate: false })
  const current = useEditorStore.getState().openFiles.get(path)
  if (!current) throw new Error('Quick editor file unavailable') // tab limit / failed read
  // Dirty memory always wins, even when its disk mtime predates the saved draft.
  // Also do not replace a buffer edited/saved/reloaded during the asynchronous load.
  const canRestore = state.unrestored.has(path) && state.drafts.get(path) === draft
  state.unrestored.delete(path)
  if (draft && canRestore && !current.isDirty && (!before || current === before)) {
    const files = new Map(useEditorStore.getState().openFiles)
    files.set(path, {
      ...current,
      content: draft.content,
      isDirty: draft.content !== current.originalContent,
      cursorPosition: draft.cursorPosition,
      scrollTop: draft.scrollTop,
      viewMode: draft.viewMode
    })
    useEditorStore.setState({ openFiles: files })
  }
  collect()
}

/** Track this entry; detached dirty buffers stay observed until saved/discarded. */
export function useQuickEditorDraftPersistence(entryId: string): void {
  useEffect(() => {
    const state = entry(entryId)
    observe()
    for (const path of tabPaths(entryId)) state.paths.add(path)
    collect()
    // Do not collect clean disk buffers before persisted drafts have been read.
    void loadDrafts(entryId)
      .then(collect)
      .catch(() => {})
    return () => {
      // Invoke flushers now (before editor unmount unregisters them), and await
      // asynchronous editors before the final snapshot and persistence flush.
      const flushing = [...state.paths].map((path) => flushEditorContent(path))
      void Promise.allSettled(flushing)
        .then(async (results) => {
          if (results.some((result) => result.status === 'rejected')) warn('editor-flush')
          // A pending read must merge untouched drafts before the final write.
          // If it fails, collect deliberately leaves the stored payload intact.
          await state.loading?.catch(() => {})
          collect()
          const result = await persistenceApi.flushPendingWrites()
          if (!result.success) warn('flush')
        })
        .catch(() => warn('flush'))
    }
  }, [entryId])
}

/** Test/session teardown; ordinary navigation intentionally does not reset ownership. */
export function resetQuickEditorDrafts(): void {
  unsubscribeEditor?.()
  unsubscribeLayouts?.()
  unsubscribeEditor = undefined
  unsubscribeLayouts = undefined
  entries.clear()
}
