import { describe, expect, expectTypeOf, it } from 'vitest'
import fixtures from '../plugin-fixtures/contracts.json'
import type { IpcResult } from './ipc.types'
import { type CommandReply, type PluginApi, parseManifest, parsePluginEvent } from './plugin.types'
import * as host from './plugin-host.types'

type FrozenHostMethodNames =
  | 'publishHostContext'
  | 'heartbeatHostContext'
  | 'getHostUi'
  | 'dismissHostUi'
  | 'executeContribution'
// Fails scoped tsc if any frozen method disappears, even without Vitest typechecking enabled.
const frozenHostMethodsPresent: Exclude<FrozenHostMethodNames, keyof PluginApi> extends never
  ? true
  : never = true

it('freezes all five PluginApi host method names and signatures', () => {
  expect(frozenHostMethodsPresent).toBe(true)
  expectTypeOf<PluginApi['publishHostContext']>().toEqualTypeOf<
    (request: host.HostContextPublishRequest) => Promise<IpcResult<host.HostContextAck>>
  >()
  expectTypeOf<PluginApi['heartbeatHostContext']>().toEqualTypeOf<
    (request: host.HostContextHeartbeatRequest) => Promise<IpcResult<host.HostContextAck>>
  >()
  expectTypeOf<PluginApi['getHostUi']>().toEqualTypeOf<
    () => Promise<IpcResult<host.HostUiSnapshot>>
  >()
  expectTypeOf<PluginApi['dismissHostUi']>().toEqualTypeOf<
    (request: host.HostUiDismissRequest) => Promise<IpcResult<host.HostUiDismissResult>>
  >()
  expectTypeOf<PluginApi['executeContribution']>().toEqualTypeOf<
    (request: host.ExecuteContributionRequest) => Promise<IpcResult<CommandReply>>
  >()
})

const uuid = '77bac08a-bcf6-43c6-b0c2-cc971d2a980e'
const context = {
  activeProject: null,
  projectIds: [],
  activeTerminalId: null,
  activeSessionId: null,
  terminals: [],
  agentSessions: [],
  focus: { textInput: false, terminal: false },
  truncated: false
}
const event = {
  topic: 'terminal.created',
  projectId: 'p1',
  data: { terminal: { id: 't1', ptyId: null, projectId: 'p1' } }
}
const meta = {
  pluginId: 'org.termul.example',
  pluginName: 'Example',
  generation: uuid,
  revision: 1,
  createdAt: 1,
  updatedAt: 2,
  expiresAt: null
}
const notification = { id: 'notice', severity: 'info', title: 'Hello' }
const status = { id: 'activity', text: 'Active' }
const ui = {
  schemaVersion: 1,
  bootId: uuid,
  revision: 1,
  notifications: [{ ...meta, ...notification }],
  progress: [{ ...meta, id: 'work', title: 'Working', state: 'running' }],
  statusItems: [
    {
      ...meta,
      ...status,
      label: 'Activity',
      location: 'statusBar',
      scope: 'window'
    }
  ]
}
const get = { requestId: 'get-1', scope: 'profile', key: 'activity' }
const delivery = {
  subscriptionId: 'activity',
  generation: uuid,
  sourceId: uuid,
  sequence: 1,
  contextRevision: 0,
  topic: 'terminal.created',
  projectId: null,
  data: null,
  resyncRequired: true
}
const execute = {
  pluginId: 'org.termul.example',
  generation: uuid,
  kind: 'menu',
  contributionId: 'activity',
  contextRevision: 0,
  target: { terminalId: 't1' }
}
const manifest = (): Record<string, any> =>
  structuredClone(fixtures.find((f) => f.kind === 'manifest' && f.valid)!.value)

const cases: [string, (v: unknown) => unknown, Record<string, unknown>][] = [
  ['context', host.parseHostContextState, context],
  [
    'callback',
    host.parseHostDeliveryRequest,
    { schemaVersion: 1, version: 1, service: 'se.host.events', method: 'deliver', input: delivery }
  ],
  ['accepted', host.parseHostAcceptedResult, { accepted: true, id: 'a' }],
  ['closed', host.parseNotificationCloseResult, { closed: true }],
  ['cleared', host.parseStatusClearResult, { cleared: true }],
  [
    'publish',
    host.parseHostContextPublishRequest,
    {
      sourceId: uuid,
      sourceSequence: 1,
      reset: true,
      snapshot: context,
      events: [event]
    }
  ],
  ['heartbeat', host.parseHostContextHeartbeatRequest, { sourceId: uuid }],
  ['ack', host.parseHostContextAck, { contextRevision: 0, sourceSequence: 1 }],
  [
    'snapshot',
    host.parseHostContextSnapshot,
    {
      sourceId: uuid,
      contextRevision: 0,
      sequences: { activity: 0 },
      scope: 'window',
      context
    }
  ],
  ['event', host.parseHostSourceEvent, event],
  ['delivery', host.parseHostEventDeliveryInput, delivery],
  [
    'deliveryReply',
    host.parseHostDeliveryReply,
    { schemaVersion: 1, result: { invalidateViews: [] } }
  ],
  ['storageGet', host.parseStorageGetRequest, get],
  ['storageSet', host.parseStorageSetRequest, { ...get, value: null, expectedRevision: 0 }],
  ['storageDelete', host.parseStorageDeleteRequest, { ...get, expectedRevision: 0 }],
  [
    'storageTicket',
    host.parseStorageTicket,
    { accepted: true, ticketId: uuid, requestId: 'get-1' }
  ],
  [
    'storageCompletion',
    host.parseStorageCompletionInput,
    {
      requestId: 'get-1',
      ticketId: uuid,
      result: { ok: true, value: { found: true, value: null, revision: 0 } }
    }
  ],
  ['notification', host.parseNotificationUpsertRequest, notification],
  ['close', host.parseNotificationCloseRequest, { id: 'notice' }],
  ['progress', host.parseProgressUpsertRequest, { id: 'work', title: 'Work', state: 'running' }],
  ['status', host.parseStatusSetRequest, status],
  ['clear', host.parseStatusClearRequest, { id: 'activity' }],
  ['ui', host.parseHostUiSnapshot, ui],
  [
    'dismiss',
    host.parseHostUiDismissRequest,
    {
      pluginId: meta.pluginId,
      generation: uuid,
      kind: 'notification',
      id: 'notice'
    }
  ],
  ['dismissResult', host.parseHostUiDismissResult, { dismissed: true }],
  ['execute', host.parseExecuteContributionRequest, execute],
  [
    'request',
    host.parseHostRequest,
    {
      schemaVersion: 1,
      service: 'se.host.context',
      version: 1,
      method: 'snapshot',
      input: { scope: 'window' }
    }
  ],
  ['reply', host.parseHostReply, { schemaVersion: 1, result: { ok: true, value: {} } }]
]

describe('strict ABI-v1 host contracts', () => {
  for (const [name, parse, value] of cases) {
    it(`${name}: accepts DTO, rejects every missing field and unknown field`, () => {
      expect(() => parse(value)).not.toThrow()
      expect(() => parse({ ...value, surprise: true })).toThrow(TypeError)
      expect(() => parse(null)).toThrow(TypeError)
      for (const key of Object.keys(value)) {
        const without = { ...value }
        delete without[key]
        expect(() => parse(without), key).toThrow(TypeError)
      }
    })
  }
  it('defaults optional presentation fields while rejecting explicit null', () => {
    expect(host.parseNotificationUpsertRequest(notification)).toMatchObject({
      actions: [],
      ttlMs: 8000
    })
    expect(host.parseStatusSetRequest(status)).toMatchObject({
      tone: 'default',
      visible: true
    })
    expect(host.parseHostUiSnapshot(ui).statusItems[0].order).toBe(0)
    for (const key of ['message', 'actions', 'ttlMs'])
      expect(() => host.parseNotificationUpsertRequest({ ...notification, [key]: null })).toThrow()
    for (const key of ['icon', 'tone', 'tooltip', 'visible', 'projectId'])
      expect(() => host.parseStatusSetRequest({ ...status, [key]: null })).toThrow()
    for (const key of ['fraction', 'cancelCommandId'])
      expect(() =>
        host.parseProgressUpsertRequest({
          id: 'a',
          title: 'A',
          state: 'running',
          [key]: null
        })
      ).toThrow()
  })
  it('uses Unicode scalar limits, safe integer revisions and exact UTF-8 value bounds', () => {
    expect(host.parseStatusSetRequest({ ...status, text: '😀'.repeat(32) }).text).toHaveLength(64)
    expect(() => host.parseStatusSetRequest({ ...status, text: '😀'.repeat(33) })).toThrow()
    expect(() => host.parseStatusSetRequest({ ...status, text: '\ud800' })).toThrow()
    expect(
      host.parseStorageSetRequest({
        ...get,
        value: 'a'.repeat(65534),
        expectedRevision: 0
      }).value
    ).toHaveLength(65534)
    expect(() =>
      host.parseStorageSetRequest({
        ...get,
        value: 'a'.repeat(65535),
        expectedRevision: 0
      })
    ).toThrow()
    expect(() =>
      host.parseStorageSetRequest({
        ...get,
        value: '😀'.repeat(16384),
        expectedRevision: 0
      })
    ).toThrow()
    expect(
      host.parseHostContextAck({
        contextRevision: Number.MAX_SAFE_INTEGER,
        sourceSequence: 1
      }).contextRevision
    ).toBe(Number.MAX_SAFE_INTEGER)
    for (const contextRevision of [Number.MAX_SAFE_INTEGER + 1, -1, 0.5, NaN, Infinity])
      expect(() => host.parseHostContextAck({ contextRevision, sourceSequence: 1 })).toThrow()
    expect(() => host.parseHostContextAck({ contextRevision: 0, sourceSequence: 0 })).toThrow()
    for (const value of [
      undefined,
      Infinity,
      new Date(),
      { missing: undefined },
      [undefined],
      new Array(1)
    ])
      expect(() => host.parseStorageSetRequest({ ...get, value, expectedRevision: 0 })).toThrow()
  })
  it('rejects non-scalar opaque IDs and measures JSON numbers using JavaScript serialization', () => {
    for (const invalid of ['\ud800', '\udfff', `p\ud800suffix`, `p\udfffsuffix`]) {
      expect(() => host.parseHostContextState({ ...context, activeTerminalId: invalid })).toThrow()
      expect(() =>
        host.parseProjectContext({ id: invalid, name: 'Project', path: '/project' })
      ).toThrow()
      expect(() =>
        host.parseStorageSetRequest({ ...get, value: { [invalid]: true }, expectedRevision: 0 })
      ).toThrow()
    }
    expect(
      host.parseHostContextState({ ...context, activeTerminalId: '😀' }).activeTerminalId
    ).toBe('😀')
    const expandedNumbers = Array(3000).fill(1e20)
    expect(new TextEncoder().encode(JSON.stringify(expandedNumbers)).length).toBe(66001)
    expect(() =>
      host.parseStorageSetRequest({ ...get, value: expandedNumbers, expectedRevision: 0 })
    ).toThrow()
    expect(host.parseHostContextAck({ sourceSequence: 1.0, contextRevision: -0 })).toEqual({
      sourceSequence: 1,
      contextRevision: -0
    })
  })
  it('enforces storage namespace shape, CAS, null values and discriminated completions', () => {
    expect(() => host.parseStorageGetRequest({ ...get, projectId: 'p1' })).toThrow()
    expect(() => host.parseStorageGetRequest({ ...get, scope: 'project' })).toThrow()
    expect(
      host.parseStorageGetRequest({ ...get, scope: 'project', projectId: 'p1' }).projectId
    ).toBe('p1')
    expect(() => host.parseStorageSetRequest({ ...get, value: null })).toThrow()
    for (const value of [
      { found: false, value: null, revision: 0 },
      { found: true, revision: 0 },
      { found: null, revision: 0 }
    ])
      expect(() =>
        host.parseStorageCompletion({
          requestId: 'r',
          ticketId: uuid,
          result: { ok: true, value }
        })
      ).toThrow()
    expect(() =>
      host.parseHostReply({
        schemaVersion: 1,
        result: {
          ok: false,
          value: {},
          error: { code: 'PLUGIN_BUSY', message: 'Busy' }
        }
      })
    ).toThrow()
    expect(
      host.parseHostReply({
        schemaVersion: 1,
        result: {
          ok: false,
          error: { code: 'PLUGIN_STORAGE_CONFLICT', message: 'Conflict' }
        }
      }).result.ok
    ).toBe(false)
  })
  it('matches event topics with exact typed payloads and resync null semantics', () => {
    expect(() => host.parseHostSourceEvent({ ...event, topic: 'project.activeChanged' })).toThrow()
    expect(() => host.parseHostSourceEvent({ ...event, data: null })).toThrow()
    expect(() => host.parseHostEventDelivery({ ...delivery, resyncRequired: false })).toThrow()
    expect(() => host.parseHostEventDelivery({ ...delivery, data: event.data })).toThrow()
    expect(
      host.parseHostEventDelivery({
        ...delivery,
        data: event.data,
        resyncRequired: false
      }).data
    ).toEqual(event.data)
    expect(() => host.parseHostContextState({ ...context, projectIds: ['p', 'p'] })).toThrow()
    expect(() =>
      host.parseHostContextState({
        ...context,
        projectIds: Array.from({ length: 257 }, (_, i) => String(i))
      })
    ).toThrow()
    expect(() =>
      host.parseHostDeliveryReply({
        schemaVersion: 1,
        result: { invalidateViews: ['v', 'v'] }
      })
    ).toThrow()
    expect(() =>
      host.parseHostContextPublishRequest({
        sourceId: uuid,
        sourceSequence: 1,
        reset: true,
        snapshot: context,
        events: Array(129).fill(event)
      })
    ).toThrow()
  })
  it('bounds UI records, attribution and typed command actions', () => {
    expect(() =>
      host.parseHostUiSnapshot({
        ...ui,
        notifications: Array(9)
          .fill(ui.notifications[0])
          .map((v, i) => ({ ...v, id: `n${i}` }))
      })
    ).toThrow()
    expect(() =>
      host.parseHostUiSnapshot({
        ...ui,
        notifications: [...ui.notifications, ...ui.notifications]
      })
    ).toThrow()
    expect(() =>
      host.parseHostUiSnapshot({
        ...ui,
        notifications: [{ ...ui.notifications[0], expiresAt: undefined }]
      })
    ).toThrow()
    expect(() =>
      host.parseHostUiSnapshot({
        ...ui,
        statusItems: [{ ...ui.statusItems[0], scope: 'activeProject' }]
      })
    ).toThrow()
    expect(() => host.parseStatusSetRequest({ ...status, icon: 'arbitrary-image' })).toThrow()
    expect(() => host.parseExecuteContributionRequest({ ...execute, actionIndex: 0 })).toThrow()
    expect(() =>
      host.parseExecuteContributionRequest({
        ...execute,
        kind: 'notification'
      })
    ).toThrow()
    expect(
      host.parseExecuteContributionRequest({
        ...execute,
        kind: 'notification',
        actionIndex: 1
      }).actionIndex
    ).toBe(1)
    expect(() =>
      host.parseExecuteContributionRequest({
        ...execute,
        target: { terminalId: null }
      })
    ).toThrow()
    expect(() =>
      host.parseNotificationUpsertRequest({
        ...notification,
        actions: [{ label: 'Go', commandId: 'go', args: 'a'.repeat(8191) }]
      })
    ).toThrow()
    for (const fraction of [-0.1, 1.1, Infinity])
      expect(() =>
        host.parseProgressUpsertRequest({
          id: 'w',
          title: 'Work',
          state: 'running',
          fraction
        })
      ).toThrow()
  })
  it('keeps host-only callback methods and secrets out of broker requests', () => {
    expect(host.isReservedHostService('se.host.secrets')).toBe(true)
    expect(host.isAvailableHostService('se.host.secrets', 1)).toBe(false)
    expect(host.isAvailableHostService('se.host.context', 2)).toBe(false)
    expect(host.PLUGIN_HOST_CAPABILITY).toBe('plugins.host-capabilities.v1')
    for (const [service, method, input] of [
      ['se.host.events', 'deliver', delivery],
      ['se.host.storage', 'completed', {}],
      ['se.host.secrets', 'get', {}]
    ])
      expect(() =>
        host.parseHostRequest({
          schemaVersion: 1,
          version: 1,
          service,
          method,
          input
        })
      ).toThrow()
    expect(
      parsePluginEvent({
        schemaVersion: 1,
        bootId: uuid,
        seq: 1,
        type: 'host_ui_changed',
        pluginId: meta.pluginId,
        generation: uuid
      }).type
    ).toBe('host_ui_changed')
  })
})

describe('additive manifest host declarations', () => {
  it('defaults arrays empty and preserves existing sidebar behavior', () => {
    const m = manifest()
    expect(parseManifest(m).contributes).toMatchObject({
      eventSubscriptions: [],
      menus: [],
      keybindings: [],
      statusItems: []
    })
    for (const key of ['eventSubscriptions', 'menus', 'keybindings', 'statusItems'])
      expect(() =>
        parseManifest({ ...m, contributes: { ...m.contributes, [key]: null } })
      ).toThrow()
    expect(() =>
      parseManifest({
        ...m,
        services: {
          ...m.services,
          provides: [{ name: 'se.host.anything', version: 1 }]
        }
      })
    ).toThrow()
  })
  it('validates cross-references, service requirements, finite predicates and shortcuts', () => {
    const m = manifest(),
      commandId = m.contributes.commands[0].id
    m.services.requires = [
      { name: 'se.host.events', version: 1 },
      { name: 'se.host.context', version: 1 },
      { name: 'se.host.status', version: 1 }
    ]
    m.contributes.eventSubscriptions = [
      { id: 'active', topic: 'project.activeChanged', scope: 'window' }
    ]
    m.contributes.menus = [
      {
        id: 'm',
        location: 'terminal.context',
        commandId,
        when: { 'terminal.exists': true }
      }
    ]
    m.contributes.keybindings = [{ id: 'k', commandId, key: '', mac: 'cmd+shift+y' }]
    m.contributes.statusItems = [
      {
        id: 's',
        label: 'Status',
        location: 'statusBar',
        scope: 'activeProject',
        commandId
      }
    ]
    expect(() => parseManifest(m)).not.toThrow()
    expect(() => parseManifest({ ...m, services: { provides: [], requires: [] } })).toThrow()
    expect(() =>
      parseManifest({
        ...m,
        contributes: {
          ...m.contributes,
          menus: [{ ...m.contributes.menus[0], commandId: 'missing' }]
        }
      })
    ).toThrow()
    for (const key of [
      'shift+a',
      'ctrl+ctrl+a',
      'shift+ctrl+a',
      'ctrl+ a',
      'CTRL+a',
      'ctrl+a ctrl+b',
      'ctrl',
      'ctrl+f13'
    ])
      expect(() => host.parsePluginKey(key), key).toThrow()
    for (const key of ['', 'ctrl+a', 'ctrl+cmd+shift+alt+f12', 'cmd+arrowleft'])
      expect(host.parsePluginKey(key)).toBe(key)
    expect(() => host.parseHostWhen({ arbitrary: true })).toThrow()
    expect(() => host.parseHostWhen({ 'terminal.exists': null })).toThrow()
    expect(() =>
      host.parsePluginKeybinding({
        id: 'k',
        commandId,
        key: '',
        when: { 'terminal.focused': true }
      })
    ).toThrow()
    expect(() =>
      host.parsePluginEventSubscription({
        id: 'a',
        topic: 'project.activeChanged',
        scope: 'activeProject'
      })
    ).toThrow()
    expect(() =>
      parseManifest({
        ...m,
        contributes: {
          ...m.contributes,
          menus: [m.contributes.menus[0], m.contributes.menus[0]]
        }
      })
    ).toThrow()
    for (const [key, limit] of [
      ['eventSubscriptions', 16],
      ['menus', 32],
      ['keybindings', 16],
      ['statusItems', 2]
    ] as const)
      expect(() =>
        parseManifest({
          ...m,
          contributes: {
            ...m.contributes,
            [key]: Array.from({ length: limit + 1 }, (_, i) => ({
              ...m.contributes[key][0],
              id: `id${i}`
            }))
          }
        })
      ).toThrow()
  })
})
