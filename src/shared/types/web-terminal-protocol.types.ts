import type { ConversationId } from './conversation.types'
import type {
  GitStatus,
  IpcResult,
  RotatedClaim,
  SpawnedTerminal,
  TerminalAttachResult,
  TerminalResumeGrant,
  TerminalResumeRequest
} from './ipc.types'

export type { TerminalResumeGrant, TerminalResumeRequest } from './ipc.types'

/** Negotiated WebSocket subprotocol enabling binary PTY output frames. */
export const WEB_TERMINAL_BINARY_PROTOCOL = 'se-terminal-v2.binary'

const WEB_TERMINAL_BINARY_MAGIC = [0x54, 0x4d, 0x4c, 0x32] as const // "TML2"
const WEB_TERMINAL_BINARY_FIXED_HEADER_BYTES = 15

export const WEB_TERMINAL_BINARY_KIND = {
  LIVE: 1,
  REPLAY: 2
} as const

export type WebTerminalBinaryKind =
  (typeof WEB_TERMINAL_BINARY_KIND)[keyof typeof WEB_TERMINAL_BINARY_KIND]

export interface WebTerminalBinaryFrame {
  kind: WebTerminalBinaryKind
  terminalId: string
  seq: number
  data: Uint8Array
}

/**
 * Decode a negotiated binary PTY frame:
 * magic[4] + kind[u8] + terminalIdLength[u16 BE] + seq[u64 BE] + id + bytes.
 */
export function decodeWebTerminalBinaryFrame(buffer: ArrayBuffer): WebTerminalBinaryFrame | null {
  if (buffer.byteLength < WEB_TERMINAL_BINARY_FIXED_HEADER_BYTES) return null
  const bytes = new Uint8Array(buffer)
  for (let index = 0; index < WEB_TERMINAL_BINARY_MAGIC.length; index++) {
    if (bytes[index] !== WEB_TERMINAL_BINARY_MAGIC[index]) return null
  }

  const kind = bytes[4]
  if (kind !== WEB_TERMINAL_BINARY_KIND.LIVE && kind !== WEB_TERMINAL_BINARY_KIND.REPLAY) {
    return null
  }

  const view = new DataView(buffer)
  const terminalIdLength = view.getUint16(5, false)
  const payloadOffset = WEB_TERMINAL_BINARY_FIXED_HEADER_BYTES + terminalIdLength
  if (terminalIdLength === 0 || payloadOffset > buffer.byteLength) return null

  const seqHigh = view.getUint32(7, false)
  const seqLow = view.getUint32(11, false)
  const seq = seqHigh * 0x1_0000_0000 + seqLow
  if (!Number.isSafeInteger(seq)) return null

  const terminalId = new TextDecoder().decode(
    bytes.subarray(WEB_TERMINAL_BINARY_FIXED_HEADER_BYTES, payloadOffset)
  )
  if (!terminalId) return null

  return {
    kind,
    terminalId,
    seq,
    data: bytes.slice(payloadOffset)
  }
}

export type WebTerminalRequestType =
  | 'authenticate'
  | 'spawn'
  | 'list'
  | 'watch'
  | 'resume'
  | 'write'
  | 'resize'
  | 'set_display_mode'
  | 'terminate'
  | 'kill'
  | 'attach'
  | 'detach'
  | 'close_view'
  | 'rotate_claim'
  | 'revoke_claim'
  | 'get_cwd'
  | 'get_git_branch'
  | 'get_git_status'
  | 'get_exit_code'
  | 'add_renderer_ref'
  | 'remove_renderer_ref'
  | 'set_protected'
  | 'update_orphan_detection'

export type TerminalCwdSource = 'workspace' | 'executionTarget'

/** Exact sanitized PTY cleanup stages emitted by both native transports. */
export const TERMINAL_CLEANUP_STAGES = ['kill', 'wait', 'flusher_join', 'reader_join'] as const

export type TerminalCleanupStage = (typeof TERMINAL_CLEANUP_STAGES)[number]

export const TERMINAL_RESOURCE_FAILURE_CODES = [
  'TERMINATE_FAILED',
  'TERMINAL_RESOURCE_ROLLBACK_FAILED'
] as const

export type TerminalResourceFailureCode = (typeof TERMINAL_RESOURCE_FAILURE_CODES)[number]

/**
 * Secret-safe recoverable resource detail. The backend deliberately omits the
 * claim, process, command, argv, cwd, environment, output, and Conversation id.
 */
export interface TerminalResourceFailureV1 {
  terminalId: string
  primaryCode: string
  cleanupStage: TerminalCleanupStage
}

/** Exact secret-free input accepted by the renderer cleanup-recovery store. */
export type TerminalCleanupRecoveryInput = Readonly<TerminalResourceFailureV1>

const SAFE_TERMINAL_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/
const SAFE_PRIMARY_CODE = /^[A-Z][A-Z0-9_]{0,127}$/

/**
 * Decode only the exact cleanup/compound error contract without rewriting the
 * original IpcResult. Callers can retain the recoverable terminal identity
 * while forwarding the stable transport envelope byte-for-byte.
 */
export function readTerminalResourceFailure(
  result: IpcResult<unknown>
): TerminalCleanupRecoveryInput | null {
  if (
    result.success ||
    !TERMINAL_RESOURCE_FAILURE_CODES.includes(result.code as TerminalResourceFailureCode)
  ) {
    return null
  }

  let value: unknown
  try {
    value = JSON.parse(result.error)
  } catch {
    return null
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null

  const record = value as Record<string, unknown>
  const keys = Object.keys(record).sort()
  if (keys.join(',') !== 'cleanupStage,primaryCode,terminalId') return null
  if (
    typeof record.terminalId !== 'string' ||
    !SAFE_TERMINAL_ID.test(record.terminalId) ||
    typeof record.primaryCode !== 'string' ||
    !SAFE_PRIMARY_CODE.test(record.primaryCode) ||
    typeof record.cleanupStage !== 'string' ||
    !TERMINAL_CLEANUP_STAGES.includes(record.cleanupStage as TerminalCleanupStage)
  ) {
    return null
  }

  return {
    terminalId: record.terminalId,
    primaryCode: record.primaryCode,
    cleanupStage: record.cleanupStage as TerminalCleanupStage
  }
}

/**
 * Remote spawn authority is intentionally narrow. The host resolves cwd from
 * the Conversation and derives shell/program/argv/environment itself.
 */
export interface TerminalSpawnIntentV1 {
  conversationId: ConversationId
  projectId?: string
  cwdSource: TerminalCwdSource
  cols: number
  rows: number
}

export type WebTerminalRequest =
  | { id: string; type: 'spawn'; payload: TerminalSpawnIntentV1 }
  | { id: string; type: 'resume'; payload: TerminalResumeRequest }
  | {
      id: string
      type: Exclude<WebTerminalRequestType, 'spawn' | 'resume'>
      payload: Record<string, unknown>
    }

export type WebTerminalReply<T = unknown> =
  | { id: string; success: true; data: T }
  | { id: string; success: false; error: string; code: string }

/** A single sequenced output chunk (live data frame). */
export interface WebTerminalDataFrame {
  type: 'data'
  terminalId: string
  seq: number
  data: number[]
}

/** Sequenced replay frame: unseen chunks sent on attach/reconnect. */
export interface WebTerminalReplayFrame {
  type: 'replay'
  terminalId: string
  chunks: Array<{ seq: number; data: number[] }>
  gap: boolean
  latestSeq: number
  snapshot: WebTerminalStateSnapshot
}

/** Gap marker frame: broadcast receiver lagged, some output was lost. */
export interface WebTerminalGapFrame {
  type: 'gap'
  terminalId: string
  lastSeq: number
}

/** Latest lifecycle/metadata state (sent with replay). */
export interface WebTerminalStateSnapshot {
  cwd: string | null
  gitBranch: string | null
  gitStatus: GitStatus | null
  exitCode: number | null
  exited: boolean
}

export type WebTerminalEventPayload =
  | { type: 'exit'; terminal_id: string; exit_code: number | null; signal: number | null }
  | { type: 'cwd_changed'; terminal_id: string; cwd: string }
  | { type: 'git_branch_changed'; terminal_id: string; branch: string | null }
  | { type: 'git_status_changed'; terminal_id: string; status: GitStatus | null }
  | { type: 'exit_code_changed'; terminal_id: string; exit_code: number }
  | {
      type: 'spawned'
      terminal_id: string
      project_id?: string | null
      conversation_id?: string | null
      cwd: string
      cols: number
      rows: number
      shell: string
    }
  | {
      type: 'display_mode_changed'
      terminal_id: string
      mode: 'phone' | 'desktop'
      cols: number
      rows: number
    }

export interface WebTerminalEventFrame {
  type: 'event'
  payload: WebTerminalEventPayload
}

export type WebTerminalFrame<T = unknown> =
  | WebTerminalReply<T>
  | WebTerminalDataFrame
  | WebTerminalReplayFrame
  | WebTerminalGapFrame
  | WebTerminalEventFrame

/**
 * CAP-3: the spawn reply carries the issued claim credential (flattened
 * camelCase, same shape as the desktop `terminal_spawn` IpcResult data).
 */
export type WebTerminalSpawnReply = WebTerminalReply<SpawnedTerminal>

/** Live host PTY listed for a companion viewer (`list`). */
export interface LiveTerminalSummary {
  id: string
  shell: string
  cwd: string
  pid: number
  cols: number
  rows: number
  conversationId?: string
  projectId?: string | null
  title: string
  gitBranch?: string | null
  displayMode?: 'phone' | 'desktop'
}

export interface WebTerminalListResult {
  terminals: LiveTerminalSummary[]
}

/** CAP-3: attach reply — shared TerminalAttachResult shape (never a claim). */
export type WebTerminalAttachReply = WebTerminalReply<TerminalAttachResult>

/** Authenticated cold-resume reply — identical to desktop TerminalResumeGrant. */
export type WebTerminalResumeReply = WebTerminalReply<TerminalResumeGrant>

/** CAP-3: rotate reply — the fresh credential. */
export type WebTerminalRotateClaimReply = WebTerminalReply<RotatedClaim>
