import { invoke } from '@tauri-apps/api/core'
import { relaunch } from '@tauri-apps/plugin-process'
import { logFrontendError } from './log-api'

/**
 * Restart the desktop GUI after the host's graceful exit cleanup.
 *
 * `relaunch()` from the process plugin cannot run that cleanup: Tauri ignores
 * `prevent_exit` for its restart code, so the old process could exit before
 * the new one was spawned and the window never came back. `restart_app` runs
 * the cleanup first, then restarts. `relaunch()` stays as the fallback for a
 * host that does not have the command.
 */
export async function restartApp(): Promise<void> {
  try {
    await invoke<void>('restart_app')
  } catch (error) {
    void logFrontendError({
      level: 'warn',
      source: 'app-restart',
      message: `stable_code=GRACEFUL_RESTART_UNAVAILABLE result=FALLBACK_RELAUNCH error=${
        error instanceof Error ? error.message : String(error)
      }`
    })
    await relaunch()
  }
}
