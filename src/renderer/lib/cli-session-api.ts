/**
 * CLI session discovery facade.
 *
 * Desktop uses Tauri IPC; web/remote uses POST /cli-sessions and
 * POST /cli-sessions/resolve. Scan lists files first; resolve lazily reads
 * the first JSONL session_id.
 */
import type {
  CliSessionApi,
  CliSessionListArgs,
  CliSessionListResult,
  CliSessionResolveArgs,
  CliSessionResolveResult
} from '@shared/types/cli-session.types'

import { createTauriCliSessionApi } from './tauri-cli-session-api'
import { isTauriContext } from './tauri-runtime'
import { webCliSessionApi } from './web-cli-session-api'

const tauriCliSessionApi = createTauriCliSessionApi()

export const cliSessionApi: CliSessionApi = {
  listSessions(args?: CliSessionListArgs): Promise<CliSessionListResult> {
    if (!isTauriContext()) return webCliSessionApi.listSessions(args)
    return tauriCliSessionApi.listSessions(args)
  },
  resolveSessions(args: CliSessionResolveArgs): Promise<CliSessionResolveResult> {
    if (!isTauriContext()) return webCliSessionApi.resolveSessions(args)
    return tauriCliSessionApi.resolveSessions(args)
  }
}

export { createTauriCliSessionApi } from './tauri-cli-session-api'
export { webCliSessionApi } from './web-cli-session-api'
