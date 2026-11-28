/**
 * Process-wide WebGL addon lifecycle counters.
 *
 * xterm's WebGL renderer is the only thing in this app that creates GPU
 * contexts, and every create/destroy pair costs a texture atlas plus a
 * compositor surface. A context that is created and never reclaimed shows up
 * as GPU-process growth with no matching renderer-side state change, which is
 * exactly the shape of the idle-churn report: `webgl_slots` stayed at 1–2 while
 * the GPU process oscillated between 61 MB and 483 MB.
 *
 * These counters exist so one `[renderer-memory]` sample can answer "did the
 * addon churn since the last sample?" without reading terminal content. They
 * are monotonically increasing totals plus the live difference, never a list.
 */

let created = 0
let disposed = 0
let contextsLost = 0

export interface WebglLifecycleSnapshot {
  /** Addons constructed since the renderer booted. */
  created: number
  /** Addons explicitly disposed since the renderer booted. */
  disposed: number
  /** `created - disposed`, clamped at zero (never negative). */
  live: number
  /** `onContextLoss` callbacks received — the browser reclaiming a context. */
  contextsLost: number
}

export function noteWebglAddonCreated(): void {
  created += 1
}

export function noteWebglAddonDisposed(): void {
  disposed += 1
}

export function noteWebglContextLost(): void {
  contextsLost += 1
}

export function getWebglLifecycleSnapshot(): WebglLifecycleSnapshot {
  return {
    created,
    disposed,
    live: Math.max(0, created - disposed),
    contextsLost
  }
}

/** @internal Testing only. */
export function _resetWebglLifecycleCountersForTesting(): void {
  created = 0
  disposed = 0
  contextsLost = 0
}
