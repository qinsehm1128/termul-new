import type { IpcResult } from '@shared/types/ipc.types'
import {
  assertConversationHistoryPage,
  assertConversationHistoryPageRequest,
  type ConversationHistoryPageV1
} from '@shared/types/web-protocol.types'
import { invoke } from '@tauri-apps/api/core'
import type { SessionIndexEntry, SessionPayload } from '@/lib/acp-history-persistence'

export interface DesktopHistoryListResult {
  sessions: SessionIndexEntry[]
  legacyImportComplete: boolean
}

export class AcpHistoryApiError extends Error {
  readonly code: string

  constructor(code: string, message: string) {
    super(message)
    this.name = 'AcpHistoryApiError'
    this.code = code
  }
}

async function invokeHistory<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  // Deliberately do not catch invoke rejections: transport errors must propagate unchanged.
  const result: IpcResult<T> = await invoke<IpcResult<T>>(command, args)
  if (!result.success) throw new AcpHistoryApiError(result.code, result.error)
  return result.data
}

export const acpHistoryApi = {
  list(): Promise<DesktopHistoryListResult> {
    return invokeHistory<DesktopHistoryListResult>('acp_history_list')
  },

  get(sessionId: string): Promise<SessionPayload | null> {
    return invokeHistory<SessionPayload | null>('acp_history_get', { sessionId })
  },

  async getPage(
    sessionId: string,
    afterSeq: number,
    limit: number,
    targetLastSeq?: number
  ): Promise<ConversationHistoryPageV1> {
    assertConversationHistoryPageRequest(afterSeq, limit, targetLastSeq)
    const args: {
      sessionId: string
      afterSeq: number
      limit: number
      targetLastSeq?: number
    } = { sessionId, afterSeq, limit }
    if (targetLastSeq !== undefined) args.targetLastSeq = targetLastSeq
    const page = await invokeHistory<ConversationHistoryPageV1>('acp_history_get_page', args)
    assertConversationHistoryPage(page, { sessionId, afterSeq, limit, targetLastSeq })
    return page
  },

  /** Legacy-store reads for the one-time KV wipe migration only. */
  listLegacy(): Promise<DesktopHistoryListResult> {
    return invokeHistory<DesktopHistoryListResult>('acp_history_list_legacy')
  },

  getLegacy(sessionId: string): Promise<SessionPayload | null> {
    return invokeHistory<SessionPayload | null>('acp_history_get_legacy', { sessionId })
  },

  async save(sessionId: string, payload: SessionPayload): Promise<void> {
    await invokeHistory<void>('acp_history_save', { sessionId, payload })
  },

  async delete(sessionId: string): Promise<void> {
    await invokeHistory<void>('acp_history_delete', { sessionId })
  },

  async flush(): Promise<void> {
    await invokeHistory<void>('acp_history_flush')
  },

  async markLegacyImportComplete(): Promise<void> {
    await invokeHistory<void>('acp_history_mark_legacy_import_complete')
  }
}
