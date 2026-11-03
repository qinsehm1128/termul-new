import { describe, expect, it } from 'vitest'
import fixtures from '../plugin-fixtures/contracts.json'
import viewIcons from '../plugin-fixtures/view-icons.json'
import {
  parseActionEvent,
  parseActivationPayload,
  parseCommandRequest,
  parseConfigPayload,
  parseConfigurationSchema,
  parseInstallRequest,
  parseManifest,
  parsePluginEvent,
  parsePluginHealth,
  parsePluginSnapshot,
  parseReloadRequest,
  parseRenderRequest,
  parseServiceReply,
  parseServiceRequest,
  parseSetConfigRequest,
  parseUiTree,
  type ViewSnapshot,
  validateActionForView,
  validateConfigValues
} from './plugin.types'
import * as host from './plugin-host.types'

const decoders: Record<string, (value: unknown) => unknown> = {
  contextSnapshotRequest: host.parseContextSnapshotRequest,
  hostUiGet: host.parseHostUiGetRequest,
  hostAccepted: host.parseHostAcceptedResult,
  notificationCloseResult: host.parseNotificationCloseResult,
  statusClearResult: host.parseStatusClearResult,
  hostRequest: host.parseHostRequest,
  hostDeliveryRequest: host.parseHostDeliveryRequest,
  hostReply: host.parseHostReply,
  hostContextPublish: host.parseHostContextPublishRequest,
  hostContextHeartbeat: host.parseHostContextHeartbeatRequest,
  hostContextAck: host.parseHostContextAck,
  hostContextState: host.parseHostContextState,
  hostContextSnapshot: host.parseHostContextSnapshot,
  hostSourceEvent: host.parseHostSourceEvent,
  hostEventDelivery: host.parseHostEventDelivery,
  hostDeliveryReply: host.parseHostDeliveryReply,
  storageGet: host.parseStorageGetRequest,
  storageSet: host.parseStorageSetRequest,
  storageDelete: host.parseStorageDeleteRequest,
  storageTicket: host.parseStorageTicket,
  storageCompletion: host.parseStorageCompletion,
  notificationUpsert: host.parseNotificationUpsertRequest,
  notificationClose: host.parseNotificationCloseRequest,
  progressUpsert: host.parseProgressUpsertRequest,
  statusSet: host.parseStatusSetRequest,
  statusClear: host.parseStatusClearRequest,
  hostUiSnapshot: host.parseHostUiSnapshot,
  hostUiDismiss: host.parseHostUiDismissRequest,
  hostUiDismissResult: host.parseHostUiDismissResult,
  executeContribution: host.parseExecuteContributionRequest,
  pluginEvent: parsePluginEvent,
  manifest: parseManifest,
  ui: parseUiTree,
  action: parseActionEvent,
  config: (value) => {
    const v = value as { schema: unknown; payload: unknown }
    return parseConfigPayload(v.payload, parseConfigurationSchema(v.schema))
  }
}
describe('native plugin shared contract fixtures (also read by Rust)', () => {
  it('rejects non-JSON opaque values before lossy serialization', () => {
    for (const result of [
      Infinity,
      NaN,
      undefined,
      () => {},
      new Date(),
      { nested: undefined },
      [undefined],
      [Infinity]
    ]) {
      expect(() => parseServiceReply({ schemaVersion: 1, result })).toThrow(TypeError)
    }
    expect(() => parseServiceReply(JSON.parse('{"schemaVersion":1,"result":1e400}'))).toThrow(
      TypeError
    )
    expect(() =>
      parseCommandRequest({
        schemaVersion: 1,
        commandId: 'reset',
        args: undefined
      })
    ).toThrow(TypeError)
    const cyclic: Record<string, unknown> = {}
    cyclic.self = cyclic
    expect(() => parseServiceReply({ schemaVersion: 1, result: cyclic })).toThrow(TypeError)
    expect(
      parseServiceReply({
        schemaVersion: 1,
        result: { nested: [null, false, 12, 'text'] }
      }).result
    ).toEqual({ nested: [null, false, 12, 'text'] })
  })
  it('decodes callback and RPC request shapes strictly', () => {
    const bootId = '77bac08a-bcf6-43c6-b0c2-cc971d2a980e'
    const config = (
      fixtures.find((x) => x.kind === 'config' && x.valid)!.value as {
        payload: unknown
      }
    ).payload
    expect(
      parseActivationPayload({
        schemaVersion: 1,
        pluginId: 'org.termul.example',
        generation: bootId,
        config
      }).config.revision
    ).toBe(1)
    expect(parseRenderRequest({ schemaVersion: 1, viewId: 'counter' }).viewId).toBe('counter')
    expect(
      parseCommandRequest({ schemaVersion: 1, commandId: 'reset', args: null }).args
    ).toBeNull()
    expect(
      parseServiceRequest({
        schemaVersion: 1,
        service: 'org.example.counter',
        version: 1,
        method: 'read',
        input: {}
      }).method
    ).toBe('read')
    expect(parseServiceReply({ schemaVersion: 1, result: false }).result).toBe(false)
    expect(parseInstallRequest({ path: '/native' }).path).toBe('/native')
    expect(parseReloadRequest({ id: 'org.termul.example' })).toEqual({
      id: 'org.termul.example'
    })
    expect(
      parseSetConfigRequest({
        id: 'org.termul.example',
        expectedRevision: 1,
        values: {}
      }).expectedRevision
    ).toBe(1)
    expect(
      parsePluginHealth({
        schemaVersion: 1,
        bootId,
        pid: 123,
        ready: true,
        safeMode: false,
        activeResources: 0
      }).ready
    ).toBe(true)
    for (const [decode, value] of [
      [parseRenderRequest, { schemaVersion: 1, viewId: 'counter', extra: true }],
      [parseCommandRequest, { schemaVersion: 1, commandId: 'reset' }],
      [parseReloadRequest, { id: 'org.termul.example', path: null }],
      [parseSetConfigRequest, { id: 'org.termul.example', expectedRevision: 0, values: {} }]
    ] as const)
      expect(() => decode(value)).toThrow()
  })
  it('accepts every host icon and at most two sidebar views alongside panels', () => {
    const manifest = parseManifest(fixtures.find((x) => x.kind === 'manifest' && x.valid)!.value)
    for (const icon of viewIcons) {
      const decoded = parseManifest({
        ...manifest,
        contributes: {
          ...manifest.contributes,
          views: [
            { id: 'first', title: 'First', location: 'sidebar', icon },
            { id: 'second', title: 'Second', location: 'sidebar' },
            { id: 'panel', title: 'Panel', location: 'panel' }
          ]
        }
      })
      expect(decoded.contributes.views[0].icon).toBe(icon)
      expect(decoded.contributes.views[1]).not.toHaveProperty('icon')
    }
  })
  for (const fixture of fixtures)
    it(fixture.name, () => {
      expect(decoders[fixture.kind], `missing fixture decoder: ${fixture.kind}`).toBeTypeOf(
        'function'
      )
      const decode = () => decoders[fixture.kind](fixture.value)
      if (fixture.valid) expect(decode).not.toThrow()
      else expect(decode).toThrow(TypeError)
    })
  it('enforces the UI depth, count, UTF-8 byte and property limits', () => {
    const leaf = { id: 'leaf', type: 'divider', props: {} }
    let root: unknown = leaf
    for (let i = 0; i < 32; i++) root = { id: `n${i}`, type: 'column', props: {}, children: [root] }
    expect(() => parseUiTree({ schemaVersion: 1, viewId: 'view', root })).toThrow()
    expect(() =>
      parseUiTree({
        schemaVersion: 1,
        viewId: 'view',
        root: {
          id: 'root',
          type: 'column',
          props: {},
          children: Array.from({ length: 1024 }, (_, i) => ({
            ...leaf,
            id: `n${i}`
          }))
        }
      })
    ).toThrow()
    expect(() => parseManifest({ ...fixtures[0].value, name: '字'.repeat(100000) })).toThrow()
    expect(() =>
      parseUiTree({
        schemaVersion: 1,
        viewId: 'view',
        root: { id: 'root', type: 'divider', props: { orientation: null } }
      })
    ).toThrow()
  })
  it('completes defaults without coercion, accepting only valid override keys', () => {
    const f = fixtures.find((x) => x.kind === 'config' && x.valid)!.value as {
      schema: unknown
    }
    const schema = parseConfigurationSchema(f.schema)
    expect(validateConfigValues(schema, {})).toEqual({
      greeting: 'Hello',
      step: 1,
      showCount: true,
      tone: 'normal'
    })
    expect(() => validateConfigValues(schema, { step: '2' })).toThrow()
    expect(() => validateConfigValues(schema, { extra: true })).toThrow()
  })
  it('normalizes specified UI defaults and rejects stale, disabled and forged actions', () => {
    const tree = parseUiTree(fixtures.find((x) => x.kind === 'ui' && x.valid)!.value)
    expect(tree.root.props).toEqual({ gap: 'sm', align: 'stretch' })
    const action = parseActionEvent(fixtures.find((x) => x.kind === 'action' && x.valid)!.value)
    const view: ViewSnapshot = {
      pluginId: action.pluginId,
      generation: action.generation,
      revision: 1,
      configRevision: 1,
      tree
    }
    expect(validateActionForView(action, view)).toEqual(action)
    for (const patch of [
      { viewRevision: 2 },
      { actionId: 'forged' },
      { nodeId: 'text' },
      { generation: '00000000-0000-0000-0000-000000000000' }
    ])
      expect(() => validateActionForView({ ...action, ...patch }, view)).toThrow()
    if ('children' in tree.root) {
      const row = tree.root.children.find((x) => x.type === 'row')!
      if ('children' in row) {
        const button = row.children[0]
        if (button.type === 'button') button.props.disabled = true
      }
    }
    expect(() => validateActionForView(action, view)).toThrow()
  })
  it('rejects unknown snapshot keys and accepts empty initial sequence', () => {
    const v = {
      schemaVersion: 1,
      bootId: '77bac08a-bcf6-43c6-b0c2-cc971d2a980e',
      seq: 0,
      plugins: []
    }
    expect(parsePluginSnapshot(v)).toEqual(v)
    expect(() => parsePluginSnapshot({ ...v, seq: Number.MAX_SAFE_INTEGER + 1 })).toThrow()
    expect(() => parsePluginSnapshot({ ...v, extra: true })).toThrow()
  })
})
