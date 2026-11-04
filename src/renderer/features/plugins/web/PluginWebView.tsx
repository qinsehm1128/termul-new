import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { type PluginWebSession, pluginApi } from '../plugin-api'
import { createWebBridge } from './bridge'

export const PLUGIN_WEB_SANDBOX =
  'allow-scripts allow-forms allow-modals allow-popups allow-downloads'
type ViewState = 'loading' | 'ready' | 'error' | 'crashed' | 'revoked'
interface Props {
  pluginId: string
  viewId: string
  title: string
  generation: string | null
  available: boolean
  crashed?: boolean
}

export function PluginWebView({
  pluginId,
  viewId,
  title,
  generation,
  available,
  crashed = false
}: Props): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const iframe = useRef<HTMLIFrameElement>(null)
  const [session, setSession] = useState<PluginWebSession | null>(null)
  const [state, setState] = useState<ViewState>('loading')
  const [reload, setReload] = useState(0)
  const loaded = useRef(false)
  const revoke = useRef<() => void>(() => {})

  // biome-ignore lint/correctness/useExhaustiveDependencies: explicit reload rotates the session capability even when its native owner is unchanged.
  useEffect(() => {
    let live = true
    let current: PluginWebSession | null = null
    let stopBridge = (): void => {}
    let closed = false
    let revoked = false
    loaded.current = false
    setSession(null)
    const close = (): void => {
      revoked = true
      stopBridge()
      if (current && !closed) {
        closed = true
        void pluginApi.closeWebSession({ sessionId: current.sessionId }).catch(() => {})
      }
    }
    const fail = (next: ViewState): void => {
      if (!live) return
      close()
      setSession(null)
      setState(next)
    }
    revoke.current = () => fail('revoked')
    if (!available || !generation) {
      setState(crashed ? 'crashed' : 'revoked')
      return () => {
        live = false
        close()
      }
    }
    setState('loading')
    const disconnect = pluginApi.onConnectionChanged((event) => {
      if (event.state !== 'connected') fail('revoked')
    })
    const changed = pluginApi.onChanged((event) => {
      if (event.pluginId && event.pluginId !== pluginId) return
      if (
        event.type === 'resync_required' ||
        event.type === 'registry_changed' ||
        event.type === 'state_changed' ||
        (event.generation && event.generation !== generation)
      )
        fail('revoked')
    })
    void Promise.all([disconnect.ready, changed.ready])
      .then(async () => {
        if (!live || revoked) return
        const result = await pluginApi.openWebSession({ id: pluginId, viewId, generation })
        if (!result.success) {
          if (live) fail('error')
          return
        }
        current = result.data
        if (!live || revoked) {
          close()
          return
        }
        // Never attach an RPC response for a different native owner or generation.
        if (
          current.pluginId !== pluginId ||
          current.viewId !== viewId ||
          current.generation !== generation
        ) {
          fail('revoked')
          return
        }
        setSession(current)
      })
      .catch(() => fail('error'))
    // Bridge mounting follows the iframe's ref commit in the separate effect below.
    const attach = (): void => {
      if (iframe.current && current && !closed && live) {
        stopBridge = createWebBridge({
          iframe: iframe.current,
          session: current,
          api: pluginApi,
          onReady: () => {
            if (live) setState('ready')
          },
          onError: (code) => fail(code.includes('EXPIRED') ? 'revoked' : 'error')
        })
      }
    }
    attachBridge.current = attach
    return () => {
      live = false
      close()
      changed()
      disconnect()
      attachBridge.current = () => {}
    }
  }, [pluginId, viewId, generation, available, crashed, reload])
  const attachBridge = useRef<() => void>(() => {})
  useEffect(() => {
    if (session) attachBridge.current()
  }, [session])

  const url = session ? new URL(session.url) : null
  // Capability fragments are not sent in HTTP requests or Referer headers.
  if (url && session)
    url.hash = new URLSearchParams({
      sessionId: session.sessionId,
      nonce: session.nonce,
      parentOrigin: window.location.origin
    }).toString()
  return (
    <section
      className="flex h-full min-h-0 flex-1 flex-col overflow-hidden text-foreground"
      aria-label={title}
      aria-busy={state === 'loading'}
    >
      <header className="flex shrink-0 items-center gap-3 border-b px-4 py-2">
        <h2 className="min-w-0 flex-1 truncate text-sm font-medium">{title}</h2>
        <span className="text-xs text-muted-foreground" title={t('web.trust')}>
          {t('web.trust')}
        </span>
        <Button
          size="sm"
          variant="outline"
          disabled={!available || !generation}
          onClick={() => setReload((value) => value + 1)}
        >
          {t('web.reload')}
        </Button>
      </header>
      {state !== 'ready' ? (
        <p
          role={state === 'loading' ? 'status' : 'alert'}
          className="p-4 text-sm text-muted-foreground"
        >
          {t(`web.${state}`)}
        </p>
      ) : null}
      {session && url ? (
        <iframe
          ref={iframe}
          title={title}
          src={url.href}
          sandbox={PLUGIN_WEB_SANDBOX}
          referrerPolicy="no-referrer"
          className="min-h-0 w-full flex-1 border-0"
          onLoad={() => {
            if (loaded.current) revoke.current()
            loaded.current = true
          }}
          onError={() => revoke.current()}
        />
      ) : null}
    </section>
  )
}
