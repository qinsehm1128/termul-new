/* se-plugin-ui v1. Classic script: no modules, dependencies, Tauri, DOM access to parent or eval. */
;(() => {
  if (window.se) return
  const fragment = new URLSearchParams(window.location.hash.slice(1))
  const boot = window.__SE_PLUGIN_UI_BOOTSTRAP__ || {}
  const sessionId = boot.sessionId || fragment.get('sessionId')
  const nonce = boot.nonce || fragment.get('nonce')
  const parentOrigin = boot.parentOrigin || fragment.get('parentOrigin')
  const parentWindow = window.parent
  const MAX_BYTES = 262144
  const TIMEOUT = 30000
  const base = { channel: 'se-plugin-ui', version: 1, sessionId, nonce }
  const pending = new Map()
  const listeners = new Map()
  let currentTheme = null
  let connected = false
  let disposed = false
  let resolveReady
  let rejectReady
  const ready = new Promise((resolve, reject) => {
    resolveReady = resolve
    rejectReady = reject
  })
  // Consumers may subscribe to ready later; avoid an unhandled rejection on unsupported embeds.
  ready.catch(() => {})
  const fault = (code, message) => Object.assign(new Error(message), { code })
  const bytes = (text) => new TextEncoder().encode(text).length
  const id = () => {
    const values = new Uint8Array(16)
    window.crypto.getRandomValues(values)
    return Array.from(values, (value) => value.toString(16).padStart(2, '0')).join('')
  }
  const isJson = (value, depth = 0) => {
    if (depth > 32) return false
    if (value === null || typeof value === 'string' || typeof value === 'boolean') return true
    if (typeof value === 'number') return Number.isFinite(value)
    if (Array.isArray(value)) return value.every((item) => isJson(item, depth + 1))
    return (
      value &&
      typeof value === 'object' &&
      Object.getPrototypeOf(value) === Object.prototype &&
      Object.values(value).every((item) => isJson(item, depth + 1))
    )
  }
  const post = (message) =>
    parentWindow.postMessage(JSON.stringify({ ...base, ...message }), parentOrigin || '*')
  const subscribe = (name, handler) => {
    if (typeof handler !== 'function') throw new TypeError('Listener must be a function')
    const set = listeners.get(name) || new Set()
    set.add(handler)
    listeners.set(name, set)
    return () => {
      set.delete(handler)
    }
  }
  const emit = (name, value) => {
    for (const handler of listeners.get(name) || []) {
      try {
        handler(value)
      } catch {
        /* One author callback must not break other subscriptions. */
      }
    }
  }
  const applyTheme = (theme) => {
    if (
      !theme ||
      typeof theme !== 'object' ||
      !Number.isSafeInteger(theme.revision) ||
      !['light', 'dark'].includes(theme.appearance) ||
      !theme.variables ||
      typeof theme.variables !== 'object' ||
      typeof theme.fontFamily !== 'string' ||
      typeof theme.fontSize !== 'string'
    )
      return
    const allowed = [
      '--background',
      '--foreground',
      '--card',
      '--card-foreground',
      '--popover',
      '--popover-foreground',
      '--primary',
      '--primary-foreground',
      '--secondary',
      '--secondary-foreground',
      '--muted',
      '--muted-foreground',
      '--accent',
      '--accent-foreground',
      '--destructive',
      '--destructive-foreground',
      '--border',
      '--input',
      '--ring',
      '--radius'
    ]
    const root = document.documentElement
    for (const key of allowed) {
      if (typeof theme.variables[key] === 'string' && theme.variables[key].length <= 256)
        root.style.setProperty(key, theme.variables[key])
    }
    root.style.setProperty('--se-font-family', theme.fontFamily.slice(0, 256))
    root.style.setProperty('--se-font-size', theme.fontSize.slice(0, 32))
    root.dataset.seTheme = theme.appearance
    root.style.colorScheme = theme.appearance
    currentTheme = theme
    emit('themeChanged', theme)
  }
  const request = async (method, params = {}) => {
    await ready
    if (disposed) throw fault('PLUGIN_UI_SESSION_EXPIRED', 'Session expired')
    if (pending.size >= 32) throw fault('PLUGIN_UI_RATE_LIMITED', 'Too many pending requests')
    if (!isJson(params)) throw fault('PLUGIN_INVALID_REQUEST', 'Expected JSON parameters')
    const requestId = id()
    const message = { ...base, type: 'request', requestId, method, params }
    if (bytes(JSON.stringify(message)) > MAX_BYTES || bytes(JSON.stringify(params)) > 65536)
      throw fault('PLUGIN_INVALID_REQUEST', 'Request too large')
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(requestId)
        reject(fault('PLUGIN_TIMEOUT', 'Request timed out; mutations may have committed'))
      }, TIMEOUT)
      pending.set(requestId, { resolve, reject, timer })
      try {
        post(message)
      } catch {
        clearTimeout(timer)
        pending.delete(requestId)
        reject(fault('PLUGIN_CORE_UNAVAILABLE', 'Parent unavailable'))
      }
    })
  }
  const teardown = () => {
    if (disposed) return
    disposed = true
    connected = false
    clearInterval(readyTimer)
    clearTimeout(handshakeTimer)
    window.removeEventListener('message', receive)
    for (const item of pending.values()) {
      clearTimeout(item.timer)
      item.reject(fault('PLUGIN_UI_SESSION_EXPIRED', 'Session expired'))
    }
    pending.clear()
    rejectReady(fault('PLUGIN_UI_SESSION_EXPIRED', 'Session expired'))
  }
  const exact = (value, keys) =>
    Object.keys(value).every((key) => keys.includes(key)) &&
    keys.every((key) => Object.hasOwn(value, key))
  const receive = (event) => {
    if (
      disposed ||
      event.source !== parentWindow ||
      (parentOrigin && event.origin !== parentOrigin) ||
      typeof event.data !== 'string' ||
      bytes(event.data) > MAX_BYTES
    )
      return
    let data
    try {
      data = JSON.parse(event.data)
    } catch {
      return
    }
    if (
      !data ||
      data.channel !== base.channel ||
      data.version !== 1 ||
      data.sessionId !== sessionId ||
      data.nonce !== nonce
    )
      return
    const keys = ['channel', 'version', 'sessionId', 'nonce', 'type']
    if (
      data.type === 'welcome' &&
      exact(data, [...keys, 'pluginId', 'viewId', 'generation', 'theme'])
    ) {
      if (connected) return
      if (
        typeof data.pluginId !== 'string' ||
        typeof data.viewId !== 'string' ||
        typeof data.generation !== 'string'
      )
        return
      applyTheme(data.theme)
      if (!currentTheme) return
      connected = true
      clearInterval(readyTimer)
      clearTimeout(handshakeTimer)
      resolveReady()
    } else if (connected && data.type === 'response' && typeof data.requestId === 'string') {
      if (
        data.ok === true
          ? !exact(data, [...keys, 'requestId', 'ok', 'result']) || !isJson(data.result)
          : data.ok !== false ||
            !exact(data, [...keys, 'requestId', 'ok', 'error']) ||
            !data.error ||
            !exact(data.error, ['code', 'message']) ||
            typeof data.error.code !== 'string' ||
            typeof data.error.message !== 'string'
      )
        return
      const item = pending.get(data.requestId)
      if (!item) return
      clearTimeout(item.timer)
      pending.delete(data.requestId)
      if (data.ok) item.resolve(data.result)
      else item.reject(fault(data.error.code, data.error.message))
    } else if (
      connected &&
      data.type === 'event' &&
      exact(data, [...keys, 'sequence', 'event', 'payload']) &&
      Number.isSafeInteger(data.sequence) &&
      data.sequence >= 0 &&
      isJson(data.payload)
    ) {
      if (data.event === 'themeChanged') applyTheme(data.payload)
      else if (
        ['message', 'invalidate', 'configChanged', 'contextEvent', 'resyncRequired'].includes(
          data.event
        )
      ) {
        emit(data.event, data.payload)
        // Invalidation normally asks authors to refresh; only lifecycle invalidation revokes.
        if (
          data.event === 'invalidate' &&
          data.payload &&
          typeof data.payload.code === 'string' &&
          data.payload.code !== 'PLUGIN_UI_INVALIDATED'
        )
          teardown()
      }
    }
  }
  const withStorageId = (params) => ({ ...params, requestId: params?.requestId || id() })
  const onMessage = (handler) => subscribe('message', handler)
  const notify = (params) => request('feedback.notification.upsert', params)
  const progress = (params) => request('feedback.progress.upsert', params)
  Object.freeze(Object.assign(progress, { upsert: progress }))
  window.se = Object.freeze({
    version: 1,
    ready,
    invoke: (command, args = {}) => request('invoke', { command, args }),
    view: Object.freeze({
      dispatch: (action, payload = null) => request('view.dispatch', { action, payload }),
      onInvalidate: (handler) => subscribe('invalidate', handler)
    }),
    config: Object.freeze({
      get: () => request('config.get'),
      set: (params) => request('config.set', params),
      onChange: (handler) => subscribe('configChanged', handler)
    }),
    context: Object.freeze({
      snapshot: (params) => request('context.snapshot', params),
      onEvent: (subscriptionId, handler) =>
        subscribe('contextEvent', (event) => {
          if (event && event.subscriptionId === subscriptionId) handler(event)
        })
    }),
    storage: Object.freeze({
      get: (params) => request('storage.get', params),
      set: (params) => request('storage.set', withStorageId(params)),
      delete: (params) => request('storage.delete', withStorageId(params))
    }),
    feedback: Object.freeze({
      notify,
      progress,
      notification: Object.freeze({
        upsert: notify,
        close: (params) => request('feedback.notification.close', params)
      })
    }),
    ui: Object.freeze({
      onMessage,
      onResyncRequired: (handler) => subscribe('resyncRequired', handler)
    }),
    onMessage,
    onInvalidate: (handler) => subscribe('invalidate', handler),
    onResyncRequired: (handler) => subscribe('resyncRequired', handler),
    theme: Object.freeze({
      onChange: (handler) => subscribe('themeChanged', handler),
      get: () => request('theme.get'),
      get current() {
        return currentTheme
      }
    })
  })
  window.addEventListener('message', receive)
  window.addEventListener('pagehide', teardown, { once: true })
  const hello = () => {
    if (!connected && !disposed && sessionId && nonce && parentOrigin && parentWindow !== window)
      post({ type: 'ready' })
  }
  const readyTimer = setInterval(hello, 250)
  const handshakeTimer = setTimeout(() => {
    rejectReady(fault('PLUGIN_UI_HANDSHAKE_TIMEOUT', 'Handshake timed out'))
    teardown()
  }, 5000)
  hello()
})()
