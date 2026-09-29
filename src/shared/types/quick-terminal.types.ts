/**
 * Quick terminals: a folder with a live shell, independent of agent sessions.
 * Mirrors `se-quick-terminal` (Rust). The host owns the record; the renderer
 * only asks it to create, open, rename, or delete one.
 */

import { isRecord } from './contract-guards'
import type { IpcResult } from './ipc.types'

export const QUICK_TERMINAL_SCHEMA_VERSION = 1 as const

export type QuickTerminalTarget =
  | { kind: 'workspace' }
  | { kind: 'project_root'; projectId: string; projectRoot: string }
  | { kind: 'worktree'; projectId: string; worktreePath: string; worktreeBranch: string }

export type QuickTerminalOrigin = 'created' | 'migrated_conversation'

export interface QuickTerminalRecord {
  schemaVersion: typeof QUICK_TERMINAL_SCHEMA_VERSION
  id: string
  title?: string
  target: QuickTerminalTarget
  /** Absolute directory the shell starts in. */
  cwd: string
  createdAtUtc: string
  updatedAtUtc: string
  /** PTY last opened for this quick terminal; may no longer be live. */
  terminalId?: string
  origin: QuickTerminalOrigin
}

export interface QuickTerminalOpened {
  record: QuickTerminalRecord
  terminalId: string
  /** Output claim when this call started the shell; absent when reused. */
  claim?: string
  spawned: boolean
}

export interface CreateQuickTerminalInput {
  target: QuickTerminalTarget
  title?: string | null
}

export interface QuickTerminalApi {
  list(): Promise<IpcResult<QuickTerminalRecord[]>>
  create(input: CreateQuickTerminalInput): Promise<IpcResult<QuickTerminalRecord>>
  open(id: string, cols: number, rows: number): Promise<IpcResult<QuickTerminalOpened>>
  rename(id: string, title: string | null): Promise<IpcResult<QuickTerminalRecord>>
  remove(id: string): Promise<IpcResult<void>>
  /** End the shell but keep the quick terminal; opening it again starts a new one. */
  close(id: string): Promise<IpcResult<QuickTerminalRecord>>
  /** Quick terminals changed outside this renderer's own calls (e.g. migration). */
  onChanged(handler: () => void): () => void
}

const INVALID = 'quick terminal is invalid'

/** TypeError so IPC decoding reports it like any other malformed payload. */
function invalid(): never {
  throw new TypeError(INVALID)
}

function requireString(value: unknown): string {
  if (typeof value !== 'string' || value.length === 0) invalid()
  return value
}

function parseTarget(value: unknown): QuickTerminalTarget {
  if (!isRecord(value)) invalid()
  switch (value.kind) {
    case 'workspace':
      return { kind: 'workspace' }
    case 'project_root':
      return {
        kind: 'project_root',
        projectId: requireString(value.projectId),
        projectRoot: requireString(value.projectRoot)
      }
    case 'worktree':
      return {
        kind: 'worktree',
        projectId: requireString(value.projectId),
        worktreePath: requireString(value.worktreePath),
        worktreeBranch: requireString(value.worktreeBranch)
      }
    default:
      return invalid()
  }
}

export function parseQuickTerminalRecord(value: unknown): QuickTerminalRecord {
  if (!isRecord(value) || value.schemaVersion !== QUICK_TERMINAL_SCHEMA_VERSION) {
    invalid()
  }
  if (value.origin !== 'created' && value.origin !== 'migrated_conversation') invalid()
  if (value.title !== undefined && typeof value.title !== 'string') invalid()
  if (value.terminalId !== undefined && typeof value.terminalId !== 'string') invalid()
  return {
    schemaVersion: QUICK_TERMINAL_SCHEMA_VERSION,
    id: requireString(value.id),
    ...(value.title !== undefined ? { title: value.title } : {}),
    target: parseTarget(value.target),
    cwd: requireString(value.cwd),
    createdAtUtc: requireString(value.createdAtUtc),
    updatedAtUtc: requireString(value.updatedAtUtc),
    ...(value.terminalId !== undefined ? { terminalId: value.terminalId } : {}),
    origin: value.origin
  }
}

export function parseQuickTerminalRecords(value: unknown): QuickTerminalRecord[] {
  if (!Array.isArray(value)) invalid()
  return value.map(parseQuickTerminalRecord)
}

export function parseQuickTerminalOpened(value: unknown): QuickTerminalOpened {
  if (!isRecord(value) || typeof value.spawned !== 'boolean') invalid()
  if (value.claim !== undefined && typeof value.claim !== 'string') invalid()
  return {
    record: parseQuickTerminalRecord(value.record),
    terminalId: requireString(value.terminalId),
    ...(value.claim !== undefined ? { claim: value.claim } : {}),
    spawned: value.spawned
  }
}
