/**
 * Renderer lifecycle breadcrumbs.
 *
 * The Rust side logs `[startup]` once per *process*. A WKWebView web-content
 * process can crash and reload without the app process restarting, which resets
 * every Zustand store — all terminal tabs disappear — while the backend log
 * shows nothing at all. Without a marker that case is indistinguishable from a
 * code path clearing the store, so neither can be ruled out from a log.
 *
 * Two markers close that gap:
 *
 * * a boot line per renderer start — a second one inside a single app process
 *   *is* the reload;
 * * a warning when the terminal list goes from non-empty to empty, which is
 *   what a store-clearing bug would look like instead.
 *
 * Whichever line shows up next time names the cause.
 */

import { logFrontendError } from './log-api'

let installed = false

/**
 * A value that is stable for one renderer lifetime and different across
 * reloads, so two boot lines can be told apart in a log.
 *
 * Not `crypto.randomUUID()` — that is absent on insecure origins, which the
 * web/remote client can be served over, and this must never throw during boot.
 */
function rendererNonce(): string {
  return Math.random().toString(36).slice(2, 10)
}

export function installRendererLifecycleMarkers(): void {
  if (installed) return
  installed = true

  const nonce = rendererNonce()
  void logFrontendError({
    level: 'warn',
    source: 'renderer-lifecycle',
    message: `renderer boot nonce=${nonce} url=${typeof location === 'undefined' ? 'n/a' : location.pathname}`
  })

  // Imported lazily so this module stays importable from `main.tsx` before the
  // app tree — and its store graph — is pulled in.
  void import('@/stores/terminal-store').then(({ useTerminalStore }) => {
    useTerminalStore.subscribe((state, previous) => {
      if (previous.terminals.length === 0 || state.terminals.length > 0) return
      void logFrontendError({
        level: 'warn',
        source: 'renderer-lifecycle',
        message: `terminal list emptied nonce=${nonce} from=${previous.terminals.length} to=0`
      })
    })
  })
}
