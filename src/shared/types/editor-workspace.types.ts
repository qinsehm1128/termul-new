/**
 * Host-discovered editor workspaces (VS Code family + Zed).
 *
 * Runtime-neutral: no `@tauri-apps/*` or `@renderer/*` imports.
 */

export type EditorWorkspaceKind = 'vscode' | 'cursor' | 'windsurf' | 'trae' | 'zed'

export type EditorWorkspaceSource = 'recent' | 'workspace-file'

export interface EditorWorkspaceCandidate {
  /** Stable checkbox key: `{editor}:{normalizedPath}`. */
  id: string
  editor: EditorWorkspaceKind
  name: string
  path: string
  source: EditorWorkspaceSource
}

export interface EditorWorkspaceList {
  candidates: EditorWorkspaceCandidate[]
}

export interface ParseCodeWorkspaceRequest {
  path: string
}

export function isEditorWorkspaceKind(value: unknown): value is EditorWorkspaceKind {
  return (
    value === 'vscode' ||
    value === 'cursor' ||
    value === 'windsurf' ||
    value === 'trae' ||
    value === 'zed'
  )
}

export function isEditorWorkspaceCandidate(value: unknown): value is EditorWorkspaceCandidate {
  if (!value || typeof value !== 'object') return false
  const record = value as Record<string, unknown>
  return (
    typeof record.id === 'string' &&
    isEditorWorkspaceKind(record.editor) &&
    typeof record.name === 'string' &&
    typeof record.path === 'string' &&
    (record.source === 'recent' || record.source === 'workspace-file')
  )
}

export function parseEditorWorkspaceList(value: unknown): EditorWorkspaceList | null {
  if (!value || typeof value !== 'object') return null
  const record = value as Record<string, unknown>
  if (!Array.isArray(record.candidates)) return null
  if (!record.candidates.every(isEditorWorkspaceCandidate)) return null
  return { candidates: record.candidates }
}
