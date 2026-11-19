/**
 * External file open facade: files the OS hands to the app.
 *
 * Desktop only. The host validates every path before it reaches this facade, so
 * callers receive absolute paths to existing files and can hand them straight to
 * the editor. Browsers have no "open with" concept and no filesystem path, so
 * every function degrades to an empty result and never touches a Tauri API
 * (`isTauriContext()` is the explicit gate, per the adapter-boundary rule).
 *
 * Two delivery paths, matching the host:
 * - `takePendingExternalFiles()` drains the queue the host filled from the cold
 *   start arguments. Its first call also tells the host the renderer is
 *   listening; later opens then arrive through `onExternalFileOpen`.
 * - `onExternalFileOpen(cb)` subscribes to `external-file-open`, emitted with
 *   `{ paths: string[] }`.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

import { logFrontendError } from './log-api'
import { cleanupTauriListener, isTauriContext } from './tauri-runtime'

/** Must match `EXTERNAL_FILE_OPEN_EVENT` in `src-tauri/src/external_open.rs`. */
export const EXTERNAL_FILE_OPEN_EVENT = 'external-file-open'

/** Command that drains the host's pending queue and marks the renderer ready. */
const TAKE_PENDING_COMMAND = 'take_pending_external_files'

/** `true` on desktop; the web/remote client has no OS file hand-off. */
export function supportsExternalFileOpen(): boolean {
  return isTauriContext()
}

function toPaths(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  return value.filter((entry): entry is string => typeof entry === 'string' && entry.length > 0)
}

/**
 * Drain the files queued before the renderer was ready.
 *
 * Returns `[]` on the web (and on any host failure): a missing hand-off must
 * never break app startup.
 */
export async function takePendingExternalFiles(): Promise<string[]> {
  if (!isTauriContext()) return []
  try {
    const paths = await invoke<unknown>(TAKE_PENDING_COMMAND)
    return toPaths(paths)
  } catch (error) {
    void logFrontendError({
      level: 'warn',
      message: `take_pending_external_files failed: ${String(error)}`,
      source: 'external-file-open-api'
    })
    return []
  }
}

/**
 * Subscribe to files opened while the app is already running.
 *
 * Returns a cleanup function; on the web it is a no-op and no listener is
 * created.
 */
export function onExternalFileOpen(callback: (paths: string[]) => void): () => void {
  if (!isTauriContext()) return () => {}

  let unlisten: Promise<UnlistenFn> | undefined
  try {
    unlisten = listen<{ paths?: unknown }>(EXTERNAL_FILE_OPEN_EVENT, ({ payload }) => {
      const paths = toPaths(payload?.paths)
      if (paths.length > 0) callback(paths)
    })
  } catch (error) {
    void logFrontendError({
      level: 'warn',
      message: `external-file-open subscribe failed: ${String(error)}`,
      source: 'external-file-open-api'
    })
    return () => {}
  }
  return () => cleanupTauriListener(unlisten)
}
