import { describe, expect, it } from 'vitest'
import fixtures from '../plugin-fixtures/contracts.json'
import { parseHostDeliveryRequest, parseHostServiceRequest, parseManifest } from './plugin.types'
import * as host from './plugin-host.types'
import * as web from './plugin-web.types'

const uuid = '77bac08a-bcf6-43c6-b0c2-cc971d2a980e'
const sha256 = 'a'.repeat(64)
const base = fixtures.find((x) => x.kind === 'manifest' && x.valid)!.value as Record<
  string,
  unknown
>
function manifest(views: unknown[], processes?: unknown[], requires: unknown[] = []): unknown {
  const parsed = parseManifest(base)
  return {
    ...parsed,
    contributes: { ...parsed.contributes, views, ...(processes ? { processes } : {}) },
    services: { provides: [], requires }
  }
}
const view = {
  id: 'main',
  title: 'Main',
  location: 'panel',
  kind: 'web',
  entry: 'index.html',
  allow: []
}
const descriptor = {
  schemaVersion: 1,
  ui: 'ui',
  native: { 'aarch64-apple-darwin': 'native/aarch64-apple-darwin/plugin.dylib' },
  files: [
    { path: 'manifest.json', size: 128, sha256 },
    { path: 'native/aarch64-apple-darwin/plugin.dylib', size: 10, sha256 },
    { path: 'ui/index.html', size: 12, sha256 }
  ]
}
const open = {
  bootId: uuid,
  pluginId: 'org.termul.example',
  viewId: 'main',
  generation: uuid,
  sessionId: uuid,
  nonce: sha256,
  packageDigest: sha256,
  entry: 'index.html',
  allow: [],
  shimVersion: 1
}
const request = { sessionId: uuid, requestId: uuid, method: 'theme.get', params: {} }
const envelope = { channel: web.PLUGIN_WEB_CHANNEL, version: 1, sessionId: uuid, nonce: sha256 }
const theme = {
  revision: 0,
  appearance: 'light',
  variables: { '--background': '#fff' },
  fontFamily: 'system-ui',
  fontSize: '14px'
}

describe('plugin web manifest and package contracts', () => {
  it('defaults omitted kind to uiTree semantically and retains legacy wire shape', () => {
    const legacy = { id: 'main', title: 'Main', location: 'panel' }
    expect(parseManifest(manifest([legacy])).contributes.views[0]).toEqual(legacy)
    expect(parseManifest(manifest([{ ...legacy, kind: 'uiTree' }])).contributes.views[0].kind).toBe(
      'uiTree'
    )
    expect(parseManifest(manifest([view])).contributes.views[0]).toEqual(view)
    for (const patch of [
      { kind: 'tree' },
      { kind: 'uiTree' },
      { entry: undefined },
      { allow: undefined },
      { allow: ['unknown'] },
      { allow: ['commands', 'commands'] },
      { extra: true }
    ])
      expect(() => parseManifest(manifest([{ ...view, ...patch }]))).toThrow(TypeError)
    expect(() => parseManifest(manifest([{ ...legacy, allow: [] }]))).toThrow(TypeError)
  })
  it('requires exact version-one host grants for allow and process declarations', () => {
    for (const [allow, service] of [
      ['storage.read', 'se.host.storage'],
      ['storage.write', 'se.host.storage'],
      ['context.read', 'se.host.context'],
      ['context.events', 'se.host.events'],
      ['feedback', 'se.host.feedback'],
      ['view.dispatch', 'se.host.ui']
    ]) {
      expect(() => parseManifest(manifest([{ ...view, allow: [allow] }]))).toThrow(TypeError)
      expect(() =>
        parseManifest(
          manifest([{ ...view, allow: [allow] }], undefined, [{ name: service, version: 1 }])
        )
      ).not.toThrow()
    }
    expect(() =>
      parseManifest(manifest([view], [{ id: 'server', command: 'bin/server' }]))
    ).toThrow(TypeError)
    expect(
      parseManifest(
        manifest(
          [view],
          [{ id: 'server', command: 'bin/server' }],
          [{ name: 'se.host.process', version: 1 }]
        )
      ).contributes.processes
    ).toHaveLength(1)
    expect(() =>
      parseManifest(manifest([view, { ...view, id: 'other', entry: 'INDEX.html' }]))
    ).toThrow(TypeError)
    expect(() =>
      parseManifest(
        manifest(
          [view],
          Array.from({ length: 5 }, (_, i) => ({ id: `p${i}`, command: 'server' })),
          [{ name: 'se.host.process', version: 1 }]
        )
      )
    ).toThrow(TypeError)
    expect(() =>
      parseManifest(
        manifest(
          [view],
          [
            { id: 'p', command: 'server' },
            { id: 'p', command: 'server' }
          ],
          [{ name: 'se.host.process', version: 1 }]
        )
      )
    ).toThrow(TypeError)
  })
  it('rejects unsafe, nonportable, reserved and oversized entry paths', () => {
    for (const path of [
      '',
      '/index.html',
      'a\\index.html',
      './index.html',
      '../index.html',
      'a/../index.html',
      'a//index.html',
      'a./index.html',
      'a /index.html',
      'a/%2e.html',
      'a/index.html?x',
      'a/index.html#x',
      'a/é.html',
      '__se__/index.html',
      '__SE__/index.html',
      'CON/index.html',
      'lpt1.txt/index.html',
      'index.htm',
      'index.HTML',
      'a'.repeat(508) + '.html'
    ])
      expect(() => web.parseWebEntry(path), path).toThrow(TypeError)
    expect(web.parseWebEntry('pages/main-panel.html')).toBe('pages/main-panel.html')
    expect(web.parseWebEntry('a'.repeat(507) + '.html')).toHaveLength(512)
  })
  it('validates descriptor closed fields, inventory limits, roots and case collisions', () => {
    expect(web.parsePackageDescriptor(descriptor)).toEqual(descriptor)
    for (const patch of [
      { schemaVersion: 2 },
      { formatVersion: 1 },
      { ui: 'assets' },
      { native: {} },
      { files: [] },
      { native: { 'aarch64-apple-darwin': 'native/other/plugin.dylib' } },
      { files: [...descriptor.files, { path: 'README.md', size: 1, sha256 }] },
      { files: [...descriptor.files, { path: 'package.json', size: 1, sha256 }] },
      { files: [...descriptor.files, { path: 'ui/INDEX.html', size: 1, sha256 }] },
      {
        files: [
          ...descriptor.files,
          { path: 'ui/large.js', size: web.PLUGIN_WEB_ASSET_MAX_BYTES + 1, sha256 }
        ]
      },
      { files: descriptor.files.map((f) => ({ ...f, sha256: sha256.toUpperCase() })) }
    ])
      expect(() => web.parsePackageDescriptor({ ...descriptor, ...patch })).toThrow(TypeError)
  })
})

describe('web RPC and string-only bridge contracts', () => {
  it('strictly validates opens, closes, identity and unknown keys', () => {
    expect(
      web.parseWebOpenRequest({ id: open.pluginId, viewId: 'main', generation: uuid }).viewId
    ).toBe('main')
    expect(web.parseWebOpenReply(open)).toEqual(open)
    for (const patch of [
      { nonce: 'secret' },
      { packageDigest: sha256.toUpperCase() },
      { sessionId: 'not-uuid' },
      { shimVersion: 2 },
      { extra: true },
      { allow: ['commands', 'commands'] }
    ])
      expect(() => web.parseWebOpenReply({ ...open, ...patch })).toThrow(TypeError)
    expect(web.parseWebCloseRequest({ sessionId: uuid })).toEqual({ sessionId: uuid })
    expect(web.parseWebCloseReply({ closed: true })).toEqual({ closed: true })
    expect(() => web.parseWebCloseReply({ closed: false })).toThrow(TypeError)
  })
  it('accepts only closed shim methods and scoped params without caller identity', () => {
    expect(web.parseWebRequest(request)).toEqual(request)
    for (const [method, params] of [
      ['invoke', { commandId: 'reset', args: null }],
      ['view.dispatch', { action: 'refresh', payload: {} }],
      ['config.get', {}],
      ['config.set', { expectedRevision: 1, values: { greeting: 'hi' } }],
      ['context.snapshot', {}],
      ['storage.get', { scope: 'profile', key: 'key' }],
      ['storage.set', { scope: 'profile', key: 'key', expectedRevision: 0, value: {} }],
      ['storage.delete', { scope: 'profile', key: 'key', expectedRevision: 0 }],
      ['feedback.notification.upsert', { id: 'note', severity: 'info', title: 'Hello' }],
      ['feedback.notification.close', { id: 'note' }],
      ['feedback.progress.upsert', { id: 'work', title: 'Work', state: 'running' }]
    ])
      expect(
        () => web.parseWebRequest({ ...request, method, params }),
        String(method)
      ).not.toThrow()
    for (const patch of [
      { method: 'plugins.list' },
      { method: 'context.subscribe' },
      { pluginId: 'other.plugin' },
      { generation: uuid },
      { requestId: 'forged/identity' },
      {
        method: 'view.dispatch',
        params: { action: 'refresh', payload: 'x'.repeat(web.PLUGIN_WEB_JSON_MAX_BYTES) }
      }
    ])
      expect(() => web.parseWebRequest({ ...request, ...patch })).toThrow(TypeError)
  })
  it('enforces bounded monotonic read sequences and asset byte/base64 consistency', () => {
    expect(web.parseWebReadRequest({ sessionId: uuid, afterSequence: 0 }).afterSequence).toBe(0)
    expect(
      web.parseWebReadReply({
        sequence: 2,
        resyncRequired: false,
        events: [
          { sequence: 1, type: 'message', payload: null },
          { sequence: 2, type: 'themeChanged', payload: theme }
        ]
      }).events
    ).toHaveLength(2)
    for (const events of [
      Array.from({ length: 17 }, (_, i) => ({
        sequence: i + 1,
        type: 'invalidate',
        payload: null
      })),
      [
        { sequence: 2, type: 'message', payload: null },
        { sequence: 1, type: 'message', payload: null }
      ],
      [{ sequence: 1, type: 'unknown', payload: null }]
    ])
      expect(() => web.parseWebReadReply({ sequence: 17, resyncRequired: false, events })).toThrow(
        TypeError
      )
    expect(web.parseWebAssetRequest({ sessionId: uuid, path: 'ui/styles/main.css' }).path).toBe(
      'ui/styles/main.css'
    )
    const asset = { mime: 'text/plain', sha256, size: 1, dataBase64: 'YQ==' }
    expect(web.parseWebAssetReply(asset)).toEqual(asset)
    for (const patch of [
      { size: 2 },
      { dataBase64: 'YQ' },
      { dataBase64: '%%%=' },
      { dataBase64: 'YR==' },
      { size: web.PLUGIN_WEB_ASSET_MAX_BYTES + 1 },
      { mime: 'text/plain\r\nInjected' }
    ])
      expect(() => web.parseWebAssetReply({ ...asset, ...patch })).toThrow(TypeError)
    const dataBase64 = Buffer.alloc(web.PLUGIN_WEB_ASSET_MAX_BYTES).toString('base64')
    expect(
      web.parseWebAssetReply({ ...asset, size: web.PLUGIN_WEB_ASSET_MAX_BYTES, dataBase64 }).size
    ).toBe(web.PLUGIN_WEB_ASSET_MAX_BYTES)
  })
  it('requires JSON strings and discriminates envelope fields for every variant', () => {
    const messages = [
      { ...envelope, type: 'ready' },
      {
        ...envelope,
        type: 'welcome',
        pluginId: open.pluginId,
        viewId: 'main',
        generation: uuid,
        theme
      },
      { ...envelope, type: 'request', requestId: uuid, method: 'theme.get', params: {} },
      { ...envelope, type: 'response', requestId: uuid, ok: true, result: null },
      {
        ...envelope,
        type: 'response',
        requestId: uuid,
        ok: false,
        error: { code: 'PLUGIN_UI_FORBIDDEN', message: 'No access' }
      },
      { ...envelope, type: 'event', sequence: 1, event: 'message', payload: {} }
    ]
    for (const message of messages) {
      expect(web.parseWebBridgeEnvelope(JSON.stringify(message))).toEqual(message)
      expect(() => web.parseWebBridgeEnvelope(message)).toThrow(TypeError)
      expect(() => web.parseWebBridgeEnvelope(JSON.stringify({ ...message, extra: true }))).toThrow(
        TypeError
      )
    }
    for (const value of [
      '{',
      JSON.stringify({ ...envelope, type: 'ready', theme }),
      JSON.stringify({ ...envelope, type: 'ready', version: 2 }),
      JSON.stringify({ ...envelope, type: 'ready', nonce: '' }),
      JSON.stringify({
        ...envelope,
        type: 'request',
        requestId: uuid,
        method: 'install',
        params: {}
      }),
      ' '.repeat(web.PLUGIN_WEB_JSON_MAX_BYTES + 1)
    ])
      expect(() => web.parseWebBridgeEnvelope(value)).toThrow(TypeError)
    expect(() => web.parseThemeSnapshot({ ...theme, variables: { '--unreviewed': 'x' } })).toThrow(
      TypeError
    )
  })
  it('rejects non-JSON opaque payloads and enforces UTF-8 bytes, not character counts', () => {
    const cyclic: Record<string, unknown> = {}
    cyclic.self = cyclic
    for (const value of [
      undefined,
      Infinity,
      NaN,
      new Date(),
      () => {},
      { nested: undefined },
      [undefined],
      cyclic,
      { [Symbol('x')]: 1 },
      '\ud800'
    ])
      expect(() => web.parseWebRequestReply({ ok: true, value })).toThrow(TypeError)
    expect(() => web.parseWebRequestReply({ ok: true, value: '字'.repeat(90000) })).toThrow(
      TypeError
    )
    expect(web.PLUGIN_WEB_CSP).toContain('https:')
    expect(web.PLUGIN_WEB_CSP).toContain("object-src 'none'")
  })
})

describe('host wake, UI delivery and managed-process contracts', () => {
  it('bounds optional wake reasons, generations, coalesced counts and distinct reason lists', () => {
    expect(host.parseHostWakeRequest({})).toEqual({})
    expect(host.parseHostWakeRequest({ reason: 'x'.repeat(64) }).reason).toHaveLength(64)
    expect(
      host.parseHostWakeDelivery({ generation: uuid, count: 2, reasons: ['a', 'b'] }).count
    ).toBe(2)
    for (const value of [
      { reason: 'x'.repeat(65) },
      { reason: null },
      { reason: 'ok', extra: true }
    ])
      expect(() => host.parseHostWakeRequest(value)).toThrow(TypeError)
    for (const patch of [
      { generation: 'bad' },
      { count: 0 },
      { count: Number.MAX_SAFE_INTEGER + 1 },
      { reasons: ['a', 'a'] },
      { reasons: Array.from({ length: 9 }, (_, i) => String(i)) }
    ])
      expect(() =>
        host.parseHostWakeDelivery({ generation: uuid, count: 10, reasons: [], ...patch })
      ).toThrow(TypeError)
  })
  it('validates UI message payload and delivered count without caller identity', () => {
    expect(host.parseHostUiPostMessageRequest({ viewId: 'main', payload: null }).payload).toBeNull()
    expect(host.parseHostUiPostMessageReply({ delivered: 16 }).delivered).toBe(16)
    for (const input of [
      { viewId: 'main', payload: 'x'.repeat(256 * 1024) },
      { viewId: 'main', payload: null, pluginId: 'forged' },
      { viewId: 'main' }
    ])
      expect(() => host.parseHostUiPostMessageRequest(input)).toThrow(TypeError)
    expect(() => host.parseHostUiPostMessageReply({ delivered: 17 })).toThrow(TypeError)
  })
  it('normalizes process defaults and accepts shell-free executable forms', () => {
    const defaults = {
      id: 'server',
      command: 'bin/server',
      args: [],
      env: {},
      autostart: true,
      restart: 'on-failure'
    }
    expect(host.parsePluginManagedProcess({ id: 'server', command: 'bin/server' })).toEqual(
      defaults
    )
    for (const command of [
      'node',
      'bin/server',
      '/usr/local/bin/server',
      'C:\\Program Files\\server.exe'
    ])
      expect(host.parsePluginManagedProcess({ id: 'server', command }).command).toBe(command)
    for (const patch of [
      { command: 'node --eval x' },
      { command: 'node;rm' },
      { command: '../server' },
      { command: 'bin/../server' },
      { command: 'bin\\server' },
      { command: 'bin/server\u0000' },
      { args: Array(65).fill('x') },
      { args: ['x'.repeat(4097)] },
      { env: { 'BAD-KEY': 'value' } },
      { env: { GOOD: 'x\u0000' } },
      { env: Object.fromEntries(Array.from({ length: 65 }, (_, i) => [`KEY${i}`, 'x'])) },
      { cwd: 'relative' },
      { autostart: 'true' },
      { restart: 'sometimes' },
      { shell: true }
    ])
      expect(() => host.parsePluginManagedProcess({ ...defaults, ...patch })).toThrow(TypeError)
  })
  it('bounds process states, pid/exit counters and finite log tails', () => {
    for (const state of ['stopped', 'starting', 'running', 'exited', 'crashed', 'backoff'])
      expect(host.parseHostProcessStatus({ state, restarts: 0 }).state).toBe(state)
    for (const patch of [
      { state: 'unknown' },
      { pid: 0 },
      { pid: null },
      { restarts: -1 },
      { exitCode: 2147483648 },
      { exitCode: -2147483649 },
      { startedAt: Number.MAX_SAFE_INTEGER + 1 }
    ])
      expect(() =>
        host.parseHostProcessStatus({ state: 'running', restarts: 0, ...patch })
      ).toThrow(TypeError)
    expect(
      host.parseHostProcessStatus({ state: 'exited', restarts: 1, exitCode: -2147483648 }).exitCode
    ).toBe(-2147483648)
    for (const tail of [0, 500])
      expect(host.parseHostProcessLogsRequest({ id: 'server', tail }).tail).toBe(tail)
    for (const tail of [-1, 501, 1.5, null])
      expect(() => host.parseHostProcessLogsRequest({ id: 'server', tail })).toThrow(TypeError)
    const line = { stream: 'stdout', at: 0, text: 'x'.repeat(4096) }
    expect(host.parseHostProcessLogsReply({ lines: [line], truncated: false }).lines).toEqual([
      line
    ])
    for (const lines of [
      [{ ...line, text: 'x'.repeat(4097) }],
      [{ ...line, stream: 'stdin' }],
      Array(501).fill({ ...line, text: '' })
    ])
      expect(() => host.parseHostProcessLogsReply({ lines, truncated: false })).toThrow(TypeError)
  })
  it('routes new services strictly and wake deliveries without registering runtime behavior', () => {
    for (const [service, method, input] of [
      ['se.host.wake', 'request', {}],
      ['se.host.ui', 'postMessage', { viewId: 'main', payload: {} }],
      ['se.host.process', 'status', { id: 'server' }],
      ['se.host.process', 'start', { id: 'server' }],
      ['se.host.process', 'stop', { id: 'server' }],
      ['se.host.process', 'restart', { id: 'server' }]
    ])
      expect(() =>
        parseHostServiceRequest({ schemaVersion: 1, version: 1, service, method, input })
      ).not.toThrow()
    expect(
      parseHostDeliveryRequest({
        schemaVersion: 1,
        version: 1,
        service: 'se.host.wake',
        method: 'wake',
        input: { generation: uuid, count: 1, reasons: [] }
      }).service
    ).toBe('se.host.wake')
    expect(() =>
      parseHostServiceRequest({
        schemaVersion: 1,
        version: 1,
        service: 'se.host.wake',
        method: 'exec',
        input: {}
      })
    ).toThrow(TypeError)
  })
})
