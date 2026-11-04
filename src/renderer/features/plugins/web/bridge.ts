import {
  type JsonValue,
  PLUGIN_WEB_CHANNEL,
  PLUGIN_WEB_JSON_MAX_BYTES,
  PLUGIN_WEB_LIMITS,
  parseWebBridgeEnvelope,
  type WebBridgeEnvelope,
  type WebEventName
} from '@shared/types/plugin-web.types'
import type { PluginWebSession, TauriPluginApi } from '../plugin-api'
import { collectTheme, observeTheme } from './theme'

type BridgeOptions = {
  iframe: HTMLIFrameElement
  session: PluginWebSession
  api: TauriPluginApi
  onReady: () => void
  onError: (code: string) => void
}

/** A per-mount capability. Disposing it prevents all late RPCs, events and replies. */
export function createWebBridge({
  iframe,
  session,
  api,
  onReady,
  onError
}: BridgeOptions): () => void {
  let active = true
  let ready = false
  let sequence = 0
  let themeSequence = 0
  let tokens: number = PLUGIN_WEB_LIMITS.burst
  let lastRefill = Date.now()
  let pollTimer: ReturnType<typeof setTimeout> | undefined
  const pending = new Map<string, ReturnType<typeof setTimeout>>()
  const seen = new Set<string>()
  const base = {
    channel: PLUGIN_WEB_CHANNEL,
    version: 1 as const,
    sessionId: session.sessionId,
    nonce: session.nonce
  }
  const send = (message: WebBridgeEnvelope): void => {
    if (!active) return
    // Validate outgoing data too. Opaque origins REQUIRE '*'; the target window is never broadcast.
    try {
      const parsed = parseWebBridgeEnvelope(JSON.stringify(message))
      if (new TextEncoder().encode(JSON.stringify(parsed)).length <= PLUGIN_WEB_JSON_MAX_BYTES) {
        iframe.contentWindow?.postMessage(JSON.stringify(parsed), '*')
      }
    } catch {
      /* Malformed host data must not be exposed to a plugin. */
    }
  }
  const event = (name: WebEventName, payload: JsonValue, eventSequence = ++themeSequence): void => {
    if (ready) send({ ...base, type: 'event', sequence: eventSequence, event: name, payload })
  }
  const errorReply = (requestId: string, code: string, message: string): void => {
    send({ ...base, type: 'response', requestId, ok: false, error: { code, message } })
  }
  const invalidate = (code: string): void => {
    if (!active) return
    event('invalidate', { code })
    dispose()
    onError(code)
  }
  const poll = async (): Promise<void> => {
    if (!active || !ready) return
    try {
      const result = await api.readWebSession({
        sessionId: session.sessionId,
        afterSequence: sequence
      })
      if (!active) return
      if (!result.success) {
        invalidate(result.code)
        return
      }
      if (result.data.resyncRequired)
        event('resyncRequired', { code: 'PLUGIN_UI_RESYNC_REQUIRED' }, result.data.sequence)
      for (const item of result.data.events) {
        if (item.sequence > sequence) event(item.type, item.payload, item.sequence)
      }
      sequence = Math.max(sequence, result.data.sequence)
    } catch {
      invalidate('PLUGIN_UI_UNAVAILABLE')
      return
    }
    if (active)
      pollTimer = setTimeout(() => {
        void poll()
      }, PLUGIN_WEB_LIMITS.pollMs)
  }
  const listener = (message: MessageEvent): void => {
    if (
      !active ||
      message.source !== iframe.contentWindow ||
      message.origin !== 'null' ||
      typeof message.data !== 'string'
    )
      return
    let envelope: WebBridgeEnvelope
    try {
      // Check byte size before walking/parsing the untrusted envelope.
      if (new TextEncoder().encode(message.data).length > PLUGIN_WEB_JSON_MAX_BYTES) return
      envelope = parseWebBridgeEnvelope(message.data)
    } catch {
      return
    }
    if (envelope.sessionId !== session.sessionId || envelope.nonce !== session.nonce) return
    if (envelope.type === 'ready') {
      if (ready) return
      ready = true
      clearTimeout(handshakeTimer)
      send({
        ...base,
        type: 'welcome',
        pluginId: session.pluginId,
        viewId: session.viewId,
        generation: session.generation,
        theme: collectTheme()
      })
      onReady()
      void poll()
      return
    }
    if (!ready || envelope.type !== 'request') return
    const { requestId, method, params } = envelope
    if (seen.has(requestId)) return
    const now = Date.now()
    tokens = Math.min(
      PLUGIN_WEB_LIMITS.burst,
      tokens + ((now - lastRefill) / 1000) * PLUGIN_WEB_LIMITS.operationsPerSecond
    )
    lastRefill = now
    if (tokens < 1 || pending.size >= PLUGIN_WEB_LIMITS.pendingRequests) {
      errorReply(requestId, 'PLUGIN_UI_RATE_LIMITED', 'Too many pending requests')
      return
    }
    if (seen.size >= 4096) {
      invalidate('PLUGIN_UI_SESSION_EXPIRED')
      return
    }
    seen.add(requestId)
    tokens -= 1
    if (method === 'theme.get') {
      send({
        ...base,
        type: 'response',
        requestId,
        ok: true,
        result: collectTheme() as unknown as JsonValue
      })
      return
    }
    const timeout = setTimeout(() => {
      pending.delete(requestId)
      errorReply(requestId, 'PLUGIN_UI_TIMEOUT', 'Request timed out')
    }, PLUGIN_WEB_LIMITS.requestTimeoutMs)
    pending.set(requestId, timeout)
    void api
      .requestWeb({ sessionId: session.sessionId, requestId, method, params })
      .then((result) => {
        if (!active || !pending.has(requestId)) return
        clearTimeout(timeout)
        pending.delete(requestId)
        if (result.success)
          send({ ...base, type: 'response', requestId, ok: true, result: result.data })
        else errorReply(requestId, result.code, result.error)
      })
      .catch(() => {
        if (!active || !pending.has(requestId)) return
        clearTimeout(timeout)
        pending.delete(requestId)
        errorReply(requestId, 'PLUGIN_UI_UNAVAILABLE', 'Host request failed')
      })
  }
  const handshakeTimer = setTimeout(() => invalidate('PLUGIN_UI_HANDSHAKE_TIMEOUT'), 5000)
  const stopTheme = observeTheme((theme) => event('themeChanged', theme as unknown as JsonValue))
  function dispose(): void {
    active = false
    window.removeEventListener('message', listener)
    clearTimeout(handshakeTimer)
    clearTimeout(pollTimer)
    stopTheme()
    for (const timer of pending.values()) clearTimeout(timer)
    pending.clear()
    seen.clear()
  }
  window.addEventListener('message', listener)
  return dispose
}
