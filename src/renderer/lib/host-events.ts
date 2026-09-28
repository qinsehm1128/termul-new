import { listen } from '@tauri-apps/api/event'
import { logFrontendError } from './log-api'
import { isTauriContext } from './tauri-runtime'

/**
 * Subscribe to an event the desktop host emits. Outside the desktop app there
 * is no push channel, so this is a no-op there. Returns the unsubscribe.
 */
export function onHostEvent<T>(name: string, handler: (payload: T) => void): () => void {
  if (!isTauriContext()) return () => undefined
  let unlisten: (() => void) | null = null
  let cancelled = false
  listen<T>(name, (event) => handler(event.payload))
    .then((stop) => {
      if (cancelled) stop()
      else unlisten = stop
    })
    .catch((error: unknown) => {
      void logFrontendError({
        level: 'warn',
        source: 'host-events',
        message: `operation=listen event=${name} error=${error instanceof Error ? error.message : String(error)}`
      })
    })
  return () => {
    cancelled = true
    unlisten?.()
  }
}
