/**
 * Boot-time instrumentation shared by both renderer entry points.
 *
 * The renderer has two entries — `main.tsx` (web client, dev server, tests) and
 * `tauri-main.tsx`, which is what `tauri-index.html` loads and therefore the
 * only one the packaged desktop app ever runs. Instrumentation wired into one
 * of them is silently absent from the other, and the bundle is no evidence
 * either way: Vite builds both entries, so the code is present in the shipped
 * assets whether or not anything calls it.
 *
 * That already cost a diagnosis. The renderer lifecycle markers were added to
 * `main.tsx` only, so the desktop build shipped `renderer boot nonce=` in its
 * bundle while the log never received a single one, and the crash the markers
 * were written to explain stayed invisible for another release.
 *
 * Everything boot-time goes here so an entry has either all of it or none of
 * it — never a subset that looks installed from the outside.
 */

import { installGlobalErrorForwarding } from './lib/log-api'
import { installRendererLifecycleMarkers } from './lib/renderer-lifecycle-markers'

/** Install every boot-time diagnostic. Call once, at the top of an entry. */
export function installBootInstrumentation(): void {
  // Forward uncaught renderer errors + unhandled rejections to the backend log
  // file so production crashes are diagnosable (issue #244).
  installGlobalErrorForwarding()

  // The backend logs `[startup]` once per *process*, so a web-content process
  // that crashes and reloads underneath a live app leaves no trace at all:
  // every in-memory store resets while the log stays silent. That made a report
  // of "all terminal tabs vanished at once" undiagnosable — it looked identical
  // to some code path clearing the store. These markers separate the two cases.
  installRendererLifecycleMarkers()
}
