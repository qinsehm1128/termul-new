/**
 * Desktop Tauri adapter for CLI session discovery.
 */
import type {
  CliSessionApi,
  CliSessionListArgs,
  CliSessionListResult,
  CliSessionResolveArgs,
  CliSessionResolveResult
} from '@shared/types/cli-session.types'
import {
  parseCliSessionListResult,
  parseCliSessionResolveResult
} from '@shared/types/cli-session.types'
import { invoke } from '@tauri-apps/api/core'

export function createTauriCliSessionApi(): CliSessionApi {
  return {
    async listSessions(args?: CliSessionListArgs): Promise<CliSessionListResult> {
      const raw = await invoke<unknown>('list_cli_sessions_cmd', { args: args ?? null })
      const parsed = parseCliSessionListResult(raw)
      if (!parsed) {
        throw new Error('CLI session scan returned an invalid payload')
      }
      return parsed
    },
    async resolveSessions(args: CliSessionResolveArgs): Promise<CliSessionResolveResult> {
      const raw = await invoke<unknown>('resolve_cli_sessions_cmd', { args })
      const parsed = parseCliSessionResolveResult(raw)
      if (!parsed) {
        throw new Error('CLI session resolve returned an invalid payload')
      }
      return parsed
    }
  }
}
