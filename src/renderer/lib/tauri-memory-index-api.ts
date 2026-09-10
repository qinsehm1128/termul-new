/**
 * Desktop Tauri adapter for the memory index.
 *
 * Mirrors `web-memory-index-api.ts` member for member; both call the same Rust
 * service, so a difference here would be a parity bug.
 */
import type {
  MemoryIndexApi,
  MemoryIndexBuildArgs,
  MemoryIndexBuildReport,
  MemoryIndexedSession,
  MemoryIndexListArgs,
  MemoryIndexScopeArgs,
  MemoryIndexSearchArgs,
  MemoryIndexSessionArgs,
  MemoryIndexStatus,
  MemorySearchResponse,
  MemorySessionDetail
} from '@shared/types/memory-index.types'
import {
  parseMemoryIndexBuildReport,
  parseMemoryIndexedSessions,
  parseMemoryIndexStatus,
  parseMemorySearchResponse,
  parseMemorySessionDetail
} from '@shared/types/memory-index.types'
import { invoke } from '@tauri-apps/api/core'

export function createTauriMemoryIndexApi(): MemoryIndexApi {
  return {
    async build(args: MemoryIndexBuildArgs): Promise<MemoryIndexBuildReport> {
      const raw = await invoke<unknown>('memory_index_build_cmd', { args })
      const parsed = parseMemoryIndexBuildReport(raw)
      if (!parsed) throw new Error('memory index build returned an invalid payload')
      return parsed
    },
    async status(args: MemoryIndexScopeArgs): Promise<MemoryIndexStatus> {
      const raw = await invoke<unknown>('memory_index_status_cmd', { args })
      const parsed = parseMemoryIndexStatus(raw)
      if (!parsed) throw new Error('memory index status returned an invalid payload')
      return parsed
    },
    async search(args: MemoryIndexSearchArgs): Promise<MemorySearchResponse> {
      const raw = await invoke<unknown>('memory_index_search_cmd', { args })
      const parsed = parseMemorySearchResponse(raw)
      if (!parsed) throw new Error('memory index search returned an invalid payload')
      return parsed
    },
    async listSessions(args: MemoryIndexListArgs): Promise<MemoryIndexedSession[]> {
      const raw = await invoke<unknown>('memory_index_sessions_cmd', { args })
      const parsed = parseMemoryIndexedSessions(raw)
      if (!parsed) throw new Error('memory index session list returned an invalid payload')
      return parsed
    },
    async getSession(args: MemoryIndexSessionArgs): Promise<MemorySessionDetail | null> {
      const raw = await invoke<unknown>('memory_index_session_cmd', { args })
      return parseMemorySessionDetail(raw)
    },
    async cancel(args: MemoryIndexScopeArgs): Promise<boolean> {
      return (await invoke<unknown>('memory_index_cancel_cmd', { args })) === true
    },
    async mcpInvocation(args: MemoryIndexScopeArgs): Promise<string[] | null> {
      const raw = await invoke<unknown>('memory_index_mcp_invocation_cmd', { args })
      if (!Array.isArray(raw) || raw.length === 0) return null
      return raw.every((part) => typeof part === 'string') ? (raw as string[]) : null
    }
  }
}
