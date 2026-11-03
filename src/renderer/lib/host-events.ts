import { listen } from '@tauri-apps/api/event'
import { logFrontendError } from './log-api'
import { isTauriContext } from './tauri-runtime'

export interface HostEventSubscription {
  (): void
  /** Resolves only after the native listener is installed; rejects on registration failure. */
  ready: Promise<void>
}

/**
 * Subscribe to an event the desktop host emits. Outside the desktop app there
 * is no push channel, so this is a no-op there. Returns the unsubscribe.
 * Callers needing a snapshot after registration can await its `ready` promise.
 */
export function onHostEvent<T>(name: string, handler: (payload: T) => void): HostEventSubscription {
  if (!isTauriContext()) return Object.assign(() => undefined, { ready: Promise.resolve() })
  let unlisten: (() => void) | null = null
  let cancelled = false
  const ready = listen<T>(name, (event) => {
    if (!cancelled) handler(event.payload)
  }).then((stop) => {
    if (cancelled) stop()
    else unlisten = stop
  })
  // Existing fire-and-forget callers still get logging without an unhandled rejection.
  void ready.catch((error: unknown) => {
    void logFrontendError({
      level: 'warn',
      source: 'host-events',
      message: `operation=listen event=${name} error=${error instanceof Error ? error.message : String(error)}`
    })
  })
  return Object.assign(
    () => {
      if (cancelled) return
      cancelled = true
      unlisten?.()
      unlisten = null
    },
    { ready }
  )
}
