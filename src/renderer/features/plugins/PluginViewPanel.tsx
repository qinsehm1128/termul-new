import {
  type PluginEvent,
  type ViewSnapshot,
  validateActionForView
} from '@shared/types/plugin.types'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { logFrontendError } from '@/lib/log-api'
import { usePluginsStore } from '@/stores/plugins-store'
import { pluginApi } from './plugin-api'
import { UiTreeRenderer } from './UiTreeRenderer'
import { isInputAction, type UiAction } from './ui-tree-rendering'
import { PluginWebView } from './web/PluginWebView'

export interface PluginViewPanelProps {
  pluginId: string
  viewId: string
}

interface PanelState {
  snapshot: ViewSnapshot | null
  loading: boolean
  offline: boolean
  error: 'view.invalid' | 'view.unavailable' | null
}

const initialState: PanelState = { snapshot: null, loading: true, offline: false, error: null }

/** Each identity gets its own queue; route changes never reuse a previous panel's operations. */
export function PluginViewPanel(props: PluginViewPanelProps): React.JSX.Element {
  const info = usePluginsStore((state) => state.plugins.find((item) => item.id === props.pluginId))
  const connected = usePluginsStore((state) => state.connected)
  const view = info?.manifest.contributes.views.find((item) => item.id === props.viewId)
  if (view?.kind === 'web') {
    return (
      <PluginWebView
        key={`${props.pluginId}/${props.viewId}`}
        {...props}
        title={view.title}
        generation={info?.generation ?? null}
        available={connected && info?.state === 'active' && info.desiredEnabled}
        crashed={info?.state === 'failed' || info?.state === 'quarantined'}
      />
    )
  }
  return <PanelSession key={`${props.pluginId}/${props.viewId}`} {...props} />
}

function PanelSession({ pluginId, viewId }: PluginViewPanelProps): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const [state, setState] = useState<PanelState>(initialState)
  const actionRef = useRef<(action: UiAction) => void>(() => {})
  const retryRef = useRef<() => void>(() => {})

  useEffect(() => {
    let alive = true
    let connected = true
    let epoch = 0
    let readId = 0
    let snapshot: ViewSnapshot | null = null
    let expectedGeneration: string | undefined
    let draining = false
    let lastEvent: { bootId: string; seq: number } | undefined
    type Queued = { action: UiAction; generation: string; epoch: number }
    const queue: Queued[] = []
    const pending = new Map<string, { item: Queued; timer: ReturnType<typeof setTimeout> }>()
    const publish = (patch: Partial<PanelState>): void => {
      if (alive) setState((previous) => ({ ...previous, ...patch }))
    }
    const clearQueue = (): void => {
      queue.length = 0
      for (const entry of pending.values()) clearTimeout(entry.timer)
      pending.clear()
    }
    const invalidate = (): void => {
      epoch++
      readId++
      clearQueue()
      snapshot = null
      publish({ snapshot: null })
    }
    const fail = (code: string): void => {
      if (code === 'PLUGIN_CORE_UNAVAILABLE') {
        connected = false
        invalidate()
        publish({ offline: true, loading: false, error: null })
      } else {
        const key = code === 'PLUGIN_INVALID_UI' ? 'view.invalid' : 'view.unavailable'
        if (code === 'PLUGIN_INVALID_UI') invalidate()
        publish({ loading: false, error: key })
      }
      void logFrontendError({
        level: 'warn',
        source: 'PluginViewPanel',
        message: 'Plugin view request failed'
      })
    }
    const accept = (next: ViewSnapshot): boolean => {
      if (next.pluginId !== pluginId || next.tree.viewId !== viewId) {
        fail('PLUGIN_INVALID_UI')
        return false
      }
      if (snapshot?.generation === next.generation && snapshot.revision > next.revision)
        return false
      snapshot = next
      expectedGeneration = next.generation
      publish({ snapshot: next, loading: false, offline: false, error: null })
      return true
    }
    const refresh = async (): Promise<void> => {
      if (!alive || !connected) return
      const currentRead = ++readId
      const currentEpoch = epoch
      publish({ loading: !snapshot })
      try {
        const result = await pluginApi.getView({ id: pluginId, viewId })
        if (!alive || !connected || currentRead !== readId || currentEpoch !== epoch) return
        if (!result.success) {
          fail(result.code)
          return
        }
        if (expectedGeneration && result.data.generation !== expectedGeneration) return
        if (snapshot && result.data.generation !== snapshot.generation) {
          invalidate()
        }
        accept(result.data)
      } catch {
        if (alive && currentRead === readId && currentEpoch === epoch) fail('PLUGIN_REQUEST_FAILED')
      }
    }
    const drain = async (): Promise<void> => {
      if (draining || !alive || !connected) return
      draining = true
      try {
        while (alive && connected && queue.length > 0) {
          const item = queue.shift()!
          const current = snapshot
          if (!current || item.epoch !== epoch || item.generation !== current.generation) continue
          const currentEpoch = epoch
          let event: ReturnType<typeof validateActionForView>
          try {
            event = validateActionForView(
              {
                schemaVersion: 1,
                eventId: crypto.randomUUID(),
                pluginId,
                viewId,
                generation: current.generation,
                viewRevision: current.revision,
                ...item.action
              },
              current
            )
          } catch {
            // The preceding action may have removed or disabled this control.
            continue
          }
          try {
            const result = await pluginApi.dispatchAction(event)
            if (!alive || !connected || currentEpoch !== epoch) continue
            if (!result.success) {
              clearQueue()
              if (
                result.code === 'PLUGIN_STALE_GENERATION' ||
                result.code === 'PLUGIN_STALE_VIEW'
              ) {
                expectedGeneration = undefined
                invalidate()
                void refresh()
              } else {
                fail(result.code)
              }
              break
            }
            // A mutation response can never introduce a new generation or replay old operations.
            if (result.data.generation !== current.generation) {
              clearQueue()
              expectedGeneration = undefined
              invalidate()
              void refresh()
              break
            }
            accept(result.data)
          } catch {
            if (alive && currentEpoch === epoch) {
              clearQueue()
              fail('PLUGIN_REQUEST_FAILED')
            }
          }
        }
      } finally {
        draining = false
      }
    }
    const enqueue = (item: Queued): void => {
      if (!alive || !connected || queue.length >= 32) return
      queue.push(item)
      void drain()
    }
    actionRef.current = (action) => {
      if (!alive || !connected || !snapshot) return
      const item = { action, generation: snapshot.generation, epoch }
      if (isInputAction(snapshot.tree, action)) {
        const previous = pending.get(action.nodeId)
        if (previous) clearTimeout(previous.timer)
        const timer = setTimeout(() => {
          pending.delete(action.nodeId)
          enqueue(item)
        }, 150)
        pending.set(action.nodeId, { item, timer })
      } else {
        // Preserve a text edit before a subsequent click/toggle in the same panel.
        for (const entry of pending.values()) {
          clearTimeout(entry.timer)
          enqueue(entry.item)
        }
        pending.clear()
        enqueue(item)
      }
    }
    retryRef.current = () => {
      connected = true
      expectedGeneration = undefined
      invalidate()
      publish({ loading: true, offline: false, error: null })
      void refresh()
    }
    const changed = (event: PluginEvent): void => {
      if (lastEvent?.bootId === event.bootId && event.seq <= lastEvent.seq) return
      const resync =
        event.type === 'resync_required' ||
        (lastEvent !== undefined &&
          (event.bootId !== lastEvent.bootId || event.seq !== lastEvent.seq + 1))
      lastEvent = { bootId: event.bootId, seq: event.seq }
      if (!resync && event.pluginId && event.pluginId !== pluginId) return
      if (!resync && event.type === 'view_changed' && event.viewId && event.viewId !== viewId)
        return
      if (
        resync ||
        event.type === 'state_changed' ||
        event.type === 'registry_changed' ||
        (event.generation && event.generation !== snapshot?.generation)
      ) {
        expectedGeneration = resync ? undefined : event.generation
        invalidate()
      }
      void refresh()
    }
    // Subscribe first so the initial get cannot miss an invalidation.
    const unsubscribe = pluginApi.onChanged(changed)
    const disconnect = pluginApi.onConnectionChanged((event) => {
      connected = event.state === 'connected'
      expectedGeneration = undefined
      lastEvent = undefined
      invalidate()
      publish({ offline: !connected, loading: connected, error: null })
      if (connected) void refresh()
    })
    void refresh()
    return () => {
      alive = false
      invalidate()
      actionRef.current = () => {}
      retryRef.current = () => {}
      unsubscribe()
      disconnect()
    }
  }, [pluginId, viewId])

  return (
    <section className="flex flex-col gap-3 text-foreground" aria-busy={state.loading}>
      {state.loading ? (
        <p role="status" className="text-muted-foreground">
          {t('view.loading')}
        </p>
      ) : null}
      {state.offline ? (
        <p role="status" className="text-muted-foreground">
          {t('state.disconnected')}
        </p>
      ) : null}
      {state.error ? (
        <p role="alert" className="text-destructive">
          {t(state.error)}
        </p>
      ) : null}
      {state.error || state.offline ? (
        <Button variant="outline" onClick={() => retryRef.current()}>
          {t('view.retry')}
        </Button>
      ) : null}
      {state.snapshot ? (
        <UiTreeRenderer
          tree={state.snapshot.tree}
          disabled={state.offline || state.loading || Boolean(state.error)}
          onAction={(action) => actionRef.current(action)}
        />
      ) : null}
    </section>
  )
}
