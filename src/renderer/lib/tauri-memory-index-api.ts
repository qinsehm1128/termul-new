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
    }
  }
}

/**
 * The command line an external MCP client should be configured with.
 *
 * Desktop-only by nature: it names this machine's executable and this host's
 * state root, neither of which a browser client can act on.
 */
export async function memoryIndexMcpInvocation(args: MemoryIndexScopeArgs): Promise<string[]> {
  const raw = await invoke<unknown>('memory_index_mcp_invocation_cmd', { args })
  return Array.isArray(raw) ? (raw as string[]) : []
}
