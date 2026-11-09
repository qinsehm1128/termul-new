import { useCallback, useEffect, useRef, useState } from 'react'
import { type RemoteDesktopState, remoteDesktopApi } from '@/lib/remote-desktop-api'
import { isTauriContext } from '@/lib/tauri-runtime'

/** Persistent local-host indicator, mounted in both renderer roots. */
export function RemoteDesktopBanner(): React.JSX.Element | null {
  const desktop = isTauriContext()
  const [state, setState] = useState<RemoteDesktopState | null>(null)
  const [stopping, setStopping] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const mounted = useRef(false)
  const busy = useRef(false)

  const stop = useCallback(async () => {
    if (!desktop || busy.current) return
    busy.current = true
    setStopping(true)
    setError(null)
    try {
      await remoteDesktopApi.emergencyStop()
      // Keep the indicator until the host publishes zero active sessions.
    } catch {
      if (mounted.current) setError('停止失败，请重试')
    } finally {
      busy.current = false
      if (mounted.current) setStopping(false)
    }
  }, [desktop])

  useEffect(() => {
    if (!desktop) return
    mounted.current = true
    let disposed = false
    let unlisten: (() => void) | undefined
    void remoteDesktopApi
      .subscribeState((next) => {
        if (!disposed) {
          setState(next)
          setError(null)
        }
      })
      .then((cleanup) => {
        if (disposed) cleanup()
        else unlisten = cleanup
      })
      .catch(() => {
        if (!disposed) setError('无法读取远程桌面状态')
      })

    // No global-shortcut plugin is installed. Capture inside the app, including
    // terminal inputs, without claiming this works while another app has focus.
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.ctrlKey &&
        event.altKey &&
        event.metaKey &&
        (event.code === 'Period' || event.key === '.') &&
        !event.repeat
      ) {
        event.preventDefault()
        event.stopPropagation()
        void stop()
      }
    }
    window.addEventListener('keydown', onKeyDown, true)
    return () => {
      disposed = true
      mounted.current = false
      unlisten?.()
      window.removeEventListener('keydown', onKeyDown, true)
    }
  }, [desktop, stop])

  if (!desktop || !state || (state.viewers <= 0 && state.controllers <= 0)) return null
  return (
    <div
      role="status"
      aria-live="polite"
      className="fixed left-1/2 top-2 z-[100] flex max-w-[calc(100vw-1rem)] -translate-x-1/2 items-center gap-2 rounded-md border border-amber-500/40 bg-background px-3 py-2 text-xs shadow-lg"
    >
      <span className="h-2 w-2 shrink-0 rounded-full bg-amber-500" aria-hidden="true" />
      <span>
        {state.controllers > 0 ? 'SeRemote 正在控制' : 'SeRemote 正在查看'}
        {state.display !== null && ` · 屏幕 ${state.display}`}
      </span>
      <span aria-hidden="true">·</span>
      <button
        type="button"
        disabled={stopping}
        onClick={() => void stop()}
        className="shrink-0 font-medium underline disabled:opacity-50"
        title="立即停止查看和控制（应用内 Ctrl+Alt+Meta+.）"
      >
        {stopping ? '停止中…' : '停止'}
      </button>
      {error && (
        <span role="alert" className="text-destructive">
          {error}
        </span>
      )}
    </div>
  )
}
