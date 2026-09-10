/**
 * Cross-agent conversation memory index facade.
 *
 * Desktop goes through Tauri IPC, web/remote through `POST /memory-index/*`.
 * Both reach the same Rust service.
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

import { createTauriMemoryIndexApi } from './tauri-memory-index-api'
import { isTauriContext } from './tauri-runtime'
import { webMemoryIndexApi } from './web-memory-index-api'

const tauriMemoryIndexApi = createTauriMemoryIndexApi()

export const memoryIndexApi: MemoryIndexApi = {
  build(args: MemoryIndexBuildArgs): Promise<MemoryIndexBuildReport> {
    if (!isTauriContext()) return webMemoryIndexApi.build(args)
    return tauriMemoryIndexApi.build(args)
  },
  status(args: MemoryIndexScopeArgs): Promise<MemoryIndexStatus> {
    if (!isTauriContext()) return webMemoryIndexApi.status(args)
    return tauriMemoryIndexApi.status(args)
  },
  search(args: MemoryIndexSearchArgs): Promise<MemorySearchResponse> {
    if (!isTauriContext()) return webMemoryIndexApi.search(args)
    return tauriMemoryIndexApi.search(args)
  },
  listSessions(args: MemoryIndexListArgs): Promise<MemoryIndexedSession[]> {
    if (!isTauriContext()) return webMemoryIndexApi.listSessions(args)
    return tauriMemoryIndexApi.listSessions(args)
  },
  getSession(args: MemoryIndexSessionArgs): Promise<MemorySessionDetail | null> {
    if (!isTauriContext()) return webMemoryIndexApi.getSession(args)
    return tauriMemoryIndexApi.getSession(args)
  },
  cancel(args: MemoryIndexScopeArgs): Promise<boolean> {
    if (!isTauriContext()) return webMemoryIndexApi.cancel(args)
    return tauriMemoryIndexApi.cancel(args)
  },
  mcpInvocation(args: MemoryIndexScopeArgs): Promise<string[] | null> {
    if (!isTauriContext()) return webMemoryIndexApi.mcpInvocation(args)
    return tauriMemoryIndexApi.mcpInvocation(args)
  }
}

export { createTauriMemoryIndexApi } from './tauri-memory-index-api'
export { webMemoryIndexApi } from './web-memory-index-api'
