import { convertFileSrc } from '@tauri-apps/api/core'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { dirname, join } from '@tauri-apps/api/path'

type MaybeUnlisten = Promise<UnlistenFn> | UnlistenFn | null | undefined

export function isTauriContext(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ !== 'undefined'
  )
}

/**
 * True when the renderer runs in the Tauri desktop webview (always local) OR is
 * served by `se-server` over a loopback origin (`localhost` / `127.0.0.1` /
 * `::1`). Used to gate web-only surfaces whose backing HTTP route is
 * loopback-guarded (e.g. worktree mutation writes) so a non-loopback LAN client
 * does not see a usable picker that would fail with `FORBIDDEN` at launch.
 */
export function isLoopbackWebClient(): boolean {
  if (isTauriContext()) return true
  if (typeof window === 'undefined' || !window.location) return false
  const host = window.location.hostname
  return host === 'localhost' || host === '127.0.0.1' || host === '::1' || host === '[::1]'
}

function isAbsolutePath(path: string): boolean {
  return /^\/?(?:[A-Za-z]:[\\/]|\/|\\\\)/.test(path)
}

export async function resolveTauriAssetPath(path: string, baseFilePath: string): Promise<string> {
  if (!isTauriContext()) return path
  const absolutePath = isAbsolutePath(path) ? path : await join(await dirname(baseFilePath), path)
  return convertFileSrc(absolutePath, 'asset')
}

export function cleanupTauriListener(unlisten: MaybeUnlisten): void {
  if (!unlisten) return

  if (typeof unlisten === 'function') {
    unlisten()
    return
  }

  if (typeof unlisten.then === 'function') {
    void unlisten
      .then((dispose) => {
        dispose()
      })
      .catch(() => {
        // Ignore teardown failures in test/browser contexts.
      })
  }
}
