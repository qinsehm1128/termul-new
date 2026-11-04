/** Runtime-neutral web UI/package contracts. No transport, filesystem or browser side effects. */

export const PLUGIN_WEB_UI_CAPABILITY = 'plugins.web-ui.v1' as const
export const PLUGIN_WEB_CHANNEL = 'se-plugin-ui' as const
export const PLUGIN_WEB_SHIM_VERSION = 1 as const
export const WEB_RPC_METHODS = {
  open: 'plugins.web.open',
  request: 'plugins.web.request',
  read: 'plugins.web.read',
  close: 'plugins.web.close',
  asset: 'plugins.web.asset'
} as const
export const WEB_SHIM_API_METHODS = [
  'se.invoke',
  'se.view.dispatch',
  'se.config.get',
  'se.config.set',
  'se.config.onChange',
  'se.context.snapshot',
  'se.context.onEvent',
  'se.storage.get',
  'se.storage.set',
  'se.storage.delete',
  'se.feedback.notification.upsert',
  'se.feedback.notification.close',
  'se.feedback.progress.upsert',
  'se.theme.get',
  'se.theme.onChange',
  'se.onMessage',
  'se.onInvalidate',
  'se.onResyncRequired'
] as const
export const PLUGIN_WEB_JSON_MAX_BYTES = 256 * 1024
export const PLUGIN_WEB_PAYLOAD_MAX_BYTES = 256 * 1024
export const PLUGIN_WEB_ASSET_MAX_BYTES = 4 * 1024 * 1024
export const PLUGIN_WEB_ASSET_REPLY_MAX_BYTES = 6 * 1024 * 1024
export const PLUGIN_PACKAGE_ARCHIVE_MAX_BYTES = 128 * 1024 * 1024
export const PLUGIN_PACKAGE_EXPANDED_MAX_BYTES = 256 * 1024 * 1024
export const PLUGIN_PACKAGE_ENTRIES_MAX = 2048
export const PLUGIN_PACKAGE_NATIVE_MAX_BYTES = 64 * 1024 * 1024
export const PLUGIN_PACKAGE_DESCRIPTOR_MAX_BYTES = 256 * 1024
export const PLUGIN_WEB_LIMITS = {
  sessions: 16,
  sessionsPerPlugin: 8,
  sessionsPerView: 2,
  leaseMs: 15000,
  pollMs: 250,
  pendingRequests: 32,
  operationsPerSecond: 20,
  burst: 40,
  requestTimeoutMs: 30000,
  queuedBytes: 256 * 1024,
  queuedEvents: 64,
  readEvents: 16
} as const
export const PLUGIN_UI_FORBIDDEN = 'PLUGIN_UI_FORBIDDEN' as const
export const PLUGIN_UI_SESSION_EXPIRED = 'PLUGIN_UI_SESSION_EXPIRED' as const
export const PLUGIN_UI_RESYNC_REQUIRED = 'PLUGIN_UI_RESYNC_REQUIRED' as const
export const PLUGIN_INVALID_PACKAGE = 'PLUGIN_INVALID_PACKAGE' as const
export const PLUGIN_PACKAGE_INTEGRITY = 'PLUGIN_PACKAGE_INTEGRITY' as const
export const WEB_VIEW_ALLOW = [
  'commands',
  'view.dispatch',
  'config.read',
  'config.write',
  'context.read',
  'context.events',
  'storage.read',
  'storage.write',
  'feedback'
] as const
export type WebViewAllow = (typeof WEB_VIEW_ALLOW)[number]
export const WEB_METHODS = [
  'invoke',
  'view.dispatch',
  'config.get',
  'config.set',
  'context.snapshot',
  'storage.get',
  'storage.set',
  'storage.delete',
  'feedback.notification.upsert',
  'feedback.notification.close',
  'feedback.progress.upsert',
  'theme.get'
] as const
export type WebMethod = (typeof WEB_METHODS)[number]
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue }
export interface PackageFile {
  path: string
  size: number
  sha256: string
}
export interface PackageDescriptor {
  schemaVersion: 1
  native: Record<string, string>
  ui: 'ui'
  files: PackageFile[]
}
export interface WebOpenRequest {
  id: string
  viewId: string
  generation: string
}
export interface WebOpenReply {
  bootId: string
  pluginId: string
  viewId: string
  generation: string
  sessionId: string
  nonce: string
  packageDigest: string
  entry: string
  allow: WebViewAllow[]
  shimVersion: 1
}
export interface WebRequest {
  sessionId: string
  requestId: string
  method: WebMethod
  params: JsonValue
}
export type WebRequestReply = JsonValue
export interface WebReadRequest {
  sessionId: string
  afterSequence: number
}
export const WEB_EVENT_NAMES = [
  'message',
  'invalidate',
  'configChanged',
  'contextEvent',
  'themeChanged',
  'resyncRequired'
] as const
export type WebEventName = (typeof WEB_EVENT_NAMES)[number]
export interface WebEvent {
  sequence: number
  type: WebEventName
  payload: JsonValue
}
export interface WebReadReply {
  sequence: number
  resyncRequired: boolean
  events: WebEvent[]
}
export interface WebCloseRequest {
  sessionId: string
}
export interface WebCloseReply {
  closed: true
}
export interface WebAssetRequest {
  sessionId: string
  path: string
}
export interface WebAssetReply {
  mime: string
  sha256: string
  size: number
  dataBase64: string
}
export const WEB_THEME_VARIABLES = [
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
] as const
export interface ThemeSnapshot {
  revision: number
  appearance: 'light' | 'dark'
  variables: Partial<Record<(typeof WEB_THEME_VARIABLES)[number], string>>
  fontFamily: string
  fontSize: string
}
type WebBridgeBase = { channel: 'se-plugin-ui'; version: 1; sessionId: string; nonce: string }
export type WebBridgeEnvelope = WebBridgeBase &
  (
    | { type: 'ready' }
    | {
        type: 'welcome'
        pluginId: string
        viewId: string
        generation: string
        theme: ThemeSnapshot
      }
    | { type: 'request'; requestId: string; method: WebMethod; params: JsonValue }
    | { type: 'response'; requestId: string; ok: true; result: JsonValue; error?: never }
    | {
        type: 'response'
        requestId: string
        ok: false
        error: { code: string; message: string }
        result?: never
      }
    | { type: 'event'; sequence: number; event: WebEventName; payload: JsonValue }
  )
const own = (o: object, k: string): boolean => Object.prototype.hasOwnProperty.call(o, k)
function bad(): never {
  throw new TypeError('Invalid plugin web contract')
}
function object(v: unknown, required: string[], optional: string[] = []): Record<string, unknown> {
  if (!v || typeof v !== 'object' || Array.isArray(v)) bad()
  const o = v as Record<string, unknown>
  if (
    required.some((k) => !own(o, k)) ||
    Object.keys(o).some((k) => !required.includes(k) && !optional.includes(k))
  )
    bad()
  return o
}
function text(v: unknown, min = 0, max = 128): string {
  if (
    typeof v !== 'string' ||
    [...v].length < min ||
    [...v].length > max ||
    /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(v)
  )
    bad()
  return v
}
function integer(v: unknown, min = 0, max = Number.MAX_SAFE_INTEGER): number {
  if (typeof v !== 'number' || !Number.isSafeInteger(v) || v < min || v > max) bad()
  return v
}
function bool(v: unknown): boolean {
  if (typeof v !== 'boolean') bad()
  return v
}
function choice<T extends string>(v: unknown, allowed: readonly T[]): T {
  if (typeof v !== 'string' || !allowed.includes(v as T)) bad()
  return v as T
}
function array(v: unknown, max: number): unknown[] {
  if (!Array.isArray(v) || v.length > max) bad()
  return v
}
function unique(v: string[]): void {
  if (new Set(v).size !== v.length) bad()
}
function literal(v: unknown): 1 {
  if (v !== 1) bad()
  return 1
}
function local(v: unknown): string {
  const s = text(v, 1)
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(s)) bad()
  return s
}
function pluginId(v: unknown): string {
  const s = text(v, 1)
  if (!/^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)+$/.test(s)) bad()
  return s
}
function uuid(v: unknown): string {
  const s = text(v, 36, 36)
  if (!/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(s)) bad()
  return s
}
function hash(v: unknown): string {
  const s = text(v, 64, 64)
  if (!/^[0-9a-f]{64}$/.test(s)) bad()
  return s
}
/** JSON.stringify must not silently coerce, omit, or invoke non-JSON object types. */
function bounded(v: unknown, max = PLUGIN_WEB_JSON_MAX_BYTES): asserts v is JsonValue {
  const seen = new Set<object>()
  function visit(x: unknown): void {
    if (x === null || typeof x === 'boolean') return
    if (typeof x === 'string') {
      text(x, 0, Number.MAX_SAFE_INTEGER)
      return
    }
    if (typeof x === 'number') {
      if (!Number.isFinite(x)) bad()
      return
    }
    if (!x || typeof x !== 'object' || seen.has(x)) bad()
    const proto = Object.getPrototypeOf(x)
    if (
      (!Array.isArray(x) && proto !== Object.prototype && proto !== null) ||
      Object.getOwnPropertySymbols(x).length
    )
      bad()
    seen.add(x)
    if (Array.isArray(x)) {
      for (let i = 0; i < x.length; i++) {
        if (!own(x, String(i))) bad()
        visit(x[i])
      }
      if (Object.keys(x).length !== x.length) bad()
    } else
      for (const [k, value] of Object.entries(x)) {
        text(k, 0, Number.MAX_SAFE_INTEGER)
        visit(value)
      }
    seen.delete(x)
  }
  try {
    visit(v)
    if (new TextEncoder().encode(JSON.stringify(v)).length > max) bad()
  } catch {
    bad()
  }
}
/** Paths are portable ASCII inventory keys, never OS paths or URLs. */
export function parsePackagePath(v: unknown): string {
  const s = text(v, 1, 512)
  const segments = s.split('/')
  if (
    segments.some(
      (p) =>
        !/^[A-Za-z0-9._-]+$/.test(p) ||
        p === '.' ||
        p === '..' ||
        p.endsWith('.') ||
        p.toLowerCase() === '__se__' ||
        /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(p)
    )
  )
    bad()
  return s
}
export function parseWebEntry(v: unknown): string {
  const s = parsePackagePath(v)
  if (!s.endsWith('.html')) bad()
  return s
}
export function parseWebViewAllow(v: unknown): WebViewAllow[] {
  const allow = array(v, WEB_VIEW_ALLOW.length).map((x) => choice(x, WEB_VIEW_ALLOW))
  unique(allow)
  return allow
}
export function parsePackageDescriptor(v: unknown): PackageDescriptor {
  bounded(v, PLUGIN_PACKAGE_DESCRIPTOR_MAX_BYTES)
  const o = object(v, ['schemaVersion', 'native', 'ui', 'files'])
  const n = object(
    o.native,
    [],
    Object.keys(o.native && typeof o.native === 'object' ? o.native : {})
  )
  if (!Object.keys(n).length || o.ui !== 'ui') bad()
  const native = Object.fromEntries(
    Object.entries(n).map(([target, path]) => {
      if (target.length > 128 || target.split('-').length < 3 || !/^[A-Za-z0-9_-]+$/.test(target))
        bad()
      const p = parsePackagePath(path)
      if (!p.startsWith(`native/${target}/`)) bad()
      return [target, p]
    })
  )
  const files = array(o.files, PLUGIN_PACKAGE_ENTRIES_MAX).map((value) => {
    const f = object(value, ['path', 'size', 'sha256'])
    const path = parsePackagePath(f.path)
    if (path !== 'manifest.json' && !path.startsWith('native/') && !path.startsWith('ui/')) bad()
    const max =
      path === 'manifest.json'
        ? PLUGIN_PACKAGE_DESCRIPTOR_MAX_BYTES
        : path.startsWith('ui/')
          ? PLUGIN_WEB_ASSET_MAX_BYTES
          : PLUGIN_PACKAGE_NATIVE_MAX_BYTES
    return { path, size: integer(f.size, 0, max), sha256: hash(f.sha256) }
  })
  unique(files.map((f) => f.path.toLowerCase()))
  if (
    files.reduce((sum, f) => sum + f.size, 0) > PLUGIN_PACKAGE_EXPANDED_MAX_BYTES ||
    !files.some((f) => f.path === 'manifest.json') ||
    Object.values(native).some((p) => !files.some((f) => f.path === p))
  )
    bad()
  return { schemaVersion: literal(o.schemaVersion), native, ui: 'ui', files }
}
export function parseWebOpenRequest(v: unknown): WebOpenRequest {
  bounded(v)
  const o = object(v, ['id', 'viewId', 'generation'])
  return { id: pluginId(o.id), viewId: local(o.viewId), generation: uuid(o.generation) }
}
export function parseWebOpenReply(v: unknown): WebOpenReply {
  bounded(v)
  const o = object(v, [
    'bootId',
    'pluginId',
    'viewId',
    'generation',
    'sessionId',
    'nonce',
    'packageDigest',
    'entry',
    'allow',
    'shimVersion'
  ])
  return {
    bootId: uuid(o.bootId),
    pluginId: pluginId(o.pluginId),
    viewId: local(o.viewId),
    generation: uuid(o.generation),
    sessionId: uuid(o.sessionId),
    nonce: hash(o.nonce),
    packageDigest: hash(o.packageDigest),
    entry: parseWebEntry(o.entry),
    allow: parseWebViewAllow(o.allow),
    shimVersion: literal(o.shimVersion)
  }
}
/** The method catalog is closed; adapter-specific parameter authorization is a runtime concern. */
export function parseWebRequest(v: unknown): WebRequest {
  bounded(v)
  const o = object(v, ['sessionId', 'requestId', 'method', 'params'])
  const method = choice(o.method, WEB_METHODS)
  bounded(o.params)
  return {
    sessionId: uuid(o.sessionId),
    requestId: asciiRequestId(o.requestId),
    method,
    params: o.params
  }
}
function asciiRequestId(v: unknown): string {
  const value = text(v, 1, 64)
  if (!/^[A-Za-z0-9._-]+$/.test(value)) bad()
  return value
}
export function parseWebRequestReply(v: unknown): WebRequestReply {
  bounded(v)
  return v
}
export function parseWebReadRequest(v: unknown): WebReadRequest {
  bounded(v)
  const o = object(v, ['sessionId', 'afterSequence'])
  return { sessionId: uuid(o.sessionId), afterSequence: integer(o.afterSequence) }
}
export function parseWebReadReply(v: unknown): WebReadReply {
  bounded(v)
  const o = object(v, ['sequence', 'resyncRequired', 'events'])
  const sequence = integer(o.sequence)
  const events = array(o.events, PLUGIN_WEB_LIMITS.readEvents).map((v) => {
    const x = object(v, ['sequence', 'type', 'payload'])
    bounded(x.payload)
    const type = choice(x.type, WEB_EVENT_NAMES)
    return { sequence: integer(x.sequence, 1, sequence), type, payload: x.payload }
  })
  for (let i = 1; i < events.length; i++) if (events[i].sequence <= events[i - 1].sequence) bad()
  return { sequence, resyncRequired: bool(o.resyncRequired), events }
}
export function parseWebCloseRequest(v: unknown): WebCloseRequest {
  bounded(v)
  const o = object(v, ['sessionId'])
  return { sessionId: uuid(o.sessionId) }
}
export function parseWebCloseReply(v: unknown): WebCloseReply {
  bounded(v)
  const o = object(v, ['closed'])
  if (o.closed !== true) bad()
  return { closed: true }
}
export function parseWebAssetRequest(v: unknown): WebAssetRequest {
  bounded(v)
  const o = object(v, ['sessionId', 'path'])
  const path = parsePackagePath(o.path)
  if (!path.startsWith('ui/')) bad()
  return { sessionId: uuid(o.sessionId), path }
}
export const PLUGIN_WEB_ASSET_MIMES = [
  'text/html',
  'text/css',
  'text/javascript',
  'application/javascript',
  'application/json',
  'image/svg+xml',
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
  'image/x-icon',
  'font/woff',
  'font/woff2',
  'text/plain',
  'application/octet-stream'
] as const
export function parseWebAssetReply(v: unknown): WebAssetReply {
  bounded(v, PLUGIN_WEB_ASSET_REPLY_MAX_BYTES)
  const o = object(v, ['mime', 'sha256', 'size', 'dataBase64'])
  const mime = choice(o.mime, PLUGIN_WEB_ASSET_MIMES)
  const size = integer(o.size, 0, PLUGIN_WEB_ASSET_MAX_BYTES)
  const dataBase64 = text(o.dataBase64, 0, 4 * Math.ceil(PLUGIN_WEB_ASSET_MAX_BYTES / 3))
  const padding = dataBase64.endsWith('==') ? 2 : dataBase64.endsWith('=') ? 1 : 0
  const body = dataBase64.slice(0, dataBase64.length - padding)
  if (
    dataBase64.length % 4 !== 0 ||
    /[^A-Za-z0-9+/]/.test(body) ||
    (dataBase64.length / 4) * 3 - padding !== size
  )
    bad()
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
  if (
    (dataBase64.endsWith('==') && (alphabet.indexOf(dataBase64.at(-3)!) & 15) !== 0) ||
    (dataBase64.endsWith('=') &&
      !dataBase64.endsWith('==') &&
      (alphabet.indexOf(dataBase64.at(-2)!) & 3) !== 0)
  )
    bad()
  return { mime, sha256: hash(o.sha256), size, dataBase64 }
}
export function parseThemeSnapshot(v: unknown): ThemeSnapshot {
  bounded(v)
  const o = object(v, ['revision', 'appearance', 'variables', 'fontFamily', 'fontSize'])
  const variables = object(o.variables, [], [...WEB_THEME_VARIABLES])
  if (!Object.keys(variables).length) bad()
  return {
    revision: integer(o.revision),
    appearance: choice(o.appearance, ['light', 'dark']),
    variables: Object.fromEntries(
      Object.entries(variables).map(([k, value]) => [k, text(value, 1, 1024)])
    ),
    fontFamily: text(o.fontFamily, 1, 512),
    fontSize: text(o.fontSize, 1, 64)
  }
}
/** Accepts only JSON wire strings, never structured-clone objects. */
export function parseWebBridgeEnvelope(wire: unknown): WebBridgeEnvelope {
  const input = text(wire, 0, PLUGIN_WEB_JSON_MAX_BYTES)
  if (new TextEncoder().encode(input).length > PLUGIN_WEB_JSON_MAX_BYTES) bad()
  let v: unknown
  try {
    v = JSON.parse(input)
  } catch {
    bad()
  }
  bounded(v)
  const fields = ['channel', 'version', 'sessionId', 'nonce', 'type']
  const o = object(v, fields, [
    'theme',
    'pluginId',
    'viewId',
    'generation',
    'requestId',
    'method',
    'params',
    'ok',
    'result',
    'error',
    'sequence',
    'event',
    'payload'
  ])
  if (o.channel !== PLUGIN_WEB_CHANNEL) bad()
  const base: WebBridgeBase = {
    channel: PLUGIN_WEB_CHANNEL,
    version: literal(o.version),
    sessionId: uuid(o.sessionId),
    nonce: hash(o.nonce)
  }
  switch (o.type) {
    case 'ready':
      object(v, fields)
      return { ...base, type: 'ready' }
    case 'welcome':
      object(v, [...fields, 'pluginId', 'viewId', 'generation', 'theme'])
      return {
        ...base,
        type: 'welcome',
        pluginId: pluginId(o.pluginId),
        viewId: local(o.viewId),
        generation: uuid(o.generation),
        theme: parseThemeSnapshot(o.theme)
      }
    case 'request': {
      object(v, [...fields, 'requestId', 'method', 'params'])
      const request = parseWebRequest({
        sessionId: o.sessionId,
        requestId: o.requestId,
        method: o.method,
        params: o.params
      })
      return {
        ...base,
        type: 'request',
        requestId: request.requestId,
        method: request.method,
        params: request.params
      }
    }
    case 'response': {
      object(v, [...fields, 'requestId', 'ok'], ['result', 'error'])
      const requestId = asciiRequestId(o.requestId)
      const ok = bool(o.ok)
      if (ok) {
        if (own(o, 'error') || !own(o, 'result')) bad()
        bounded(o.result)
        return { ...base, type: 'response', requestId, ok, result: o.result }
      }
      if (own(o, 'result')) bad()
      const error = object(o.error, ['code', 'message'])
      return {
        ...base,
        type: 'response',
        requestId,
        ok,
        error: { code: text(error.code, 1, 128), message: text(error.message, 0, 2048) }
      }
    }
    case 'event': {
      object(v, [...fields, 'sequence', 'event', 'payload'])
      bounded(o.payload)
      return {
        ...base,
        type: 'event',
        sequence: integer(o.sequence, 1),
        event: choice(o.event, WEB_EVENT_NAMES),
        payload: o.payload
      }
    }
    default:
      return bad()
  }
}
/** Relaxed child CSP per user override; isolation still depends on the opaque sandbox and IPC gates. */
export const PLUGIN_WEB_CSP =
  "default-src 'self' se-plugin: http: https: ws: wss: data: blob:; script-src 'self' 'unsafe-inline' 'unsafe-eval' se-plugin: http: https: data: blob:; style-src 'self' 'unsafe-inline' se-plugin: http: https: data: blob:; connect-src http: https: ws: wss: data: blob:; img-src 'self' se-plugin: http: https: ws: wss: data: blob:; font-src 'self' se-plugin: http: https: ws: wss: data: blob:; media-src 'self' se-plugin: http: https: ws: wss: data: blob:; frame-src http: https: ws: wss: data: blob:; object-src 'none'; base-uri 'none'; form-action 'none'" as const
