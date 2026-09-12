import { logFrontendError } from './log-api'

/**
 * Longest the app-close path waits for any single persistence flush.
 *
 * `onCloseRequested` has already prevented the close by the time these run, so
 * the window is up and unresponsive for however long they take. Each stage
 * gets its OWN budget rather than sharing one, so a single slow writer cannot
 * consume the whole allowance and starve the rest.
 *
 * Deliberately not tuned against the host's 30s `DEFAULT_DRAIN_TIMEOUT`: that
 * budget covers durable host-side draining and should only be changed off
 * per-stage timing telemetry, not off a renderer-side guess.
 */
export const CLOSE_FLUSH_TIMEOUT_MS = 3000

export type CloseFlushStage = 'app-settings' | 'pending-writes' | 'session-index' | 'acp-history'

export type CloseFlushOutcome = 'done' | 'timeout' | 'failed'

/**
 * Run one close-path flush under a bounded timeout.
 *
 * Never rejects: a flush that times out or throws degrades the close instead
 * of blocking it, and says so with a stable code. Losing the tail of a
 * debounced write is recoverable; a window that never closes is not.
 *
 * The timeout does not cancel the underlying work — nothing here can — it only
 * stops the close path from waiting on it.
 */
export async function runCloseFlush(
  stage: CloseFlushStage,
  work: Promise<unknown>,
  timeoutMs: number = CLOSE_FLUSH_TIMEOUT_MS
): Promise<CloseFlushOutcome> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    const outcome = await Promise.race([
      work.then((): CloseFlushOutcome => 'done'),
      new Promise<CloseFlushOutcome>((resolve) => {
        timer = setTimeout(() => resolve('timeout'), timeoutMs)
      })
    ])
    if (outcome === 'timeout') {
      void logFrontendError({
        level: 'error',
        source: 'workspace-close.flush',
        message: `stage=${stage} stable_code=CLOSE_FLUSH_TIMEOUT result=DEGRADED timeout_ms=${timeoutMs}`
      })
    }
    return outcome
  } catch (error) {
    void logFrontendError({
      level: 'error',
      source: 'workspace-close.flush',
      message: `stage=${stage} stable_code=CLOSE_FLUSH_FAILED result=DEGRADED error=${
        error instanceof Error ? error.message : String(error)
      }`
    })
    return 'failed'
  } finally {
    if (timer) clearTimeout(timer)
  }
}
