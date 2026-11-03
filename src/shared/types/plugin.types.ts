/** Runtime-neutral native plugin v1 contracts; mirrored by se-plugin-protocol. */
import viewIcons from '../plugin-fixtures/view-icons.json'
import type { IpcResult } from './ipc.types'
export const PLUGIN_SCHEMA_VERSION = 1 as const
export const PLUGIN_JSON_MAX_BYTES = 256 * 1024
export type ConfigValue = string | number | boolean
export type ConfigValues = Record<string, ConfigValue>
export interface ConfigurationProperty {
  type: 'string' | 'number' | 'boolean'
  title: string
  description: string
  default: ConfigValue
  enum?: ConfigValue[]
  enumLabels?: string[]
  minLength?: number
  maxLength?: number
  minimum?: number
  maximum?: number
}
export interface ConfigurationSchema {
  title: string
  properties: Record<string, ConfigurationProperty>
}
export interface PluginCommand {
  id: string
  title: string
  description?: string
}
export interface PluginView {
  id: string
  title: string
  location: 'panel' | 'sidebar'
  icon?: string
}
export interface PluginService {
  name: string
  version: number
}
export interface Manifest {
  schemaVersion: 1
  id: string
  name: string
  version: string
  abi: { min: number; max: number }
  contributes: {
    commands: PluginCommand[]
    configuration: ConfigurationSchema
    views: PluginView[]
  }
  services: { provides: PluginService[]; requires: PluginService[] }
}
export type PluginManifest = Manifest
export interface ConfigPayload {
  schemaVersion: 1
  pluginId: string
  revision: number
  values: ConfigValues
  changedKeys: string[]
}
export interface ActivationPayload {
  schemaVersion: 1
  pluginId: string
  generation: string
  config: ConfigPayload
}
export interface RenderRequest {
  schemaVersion: 1
  viewId: string
}
export interface CommandRequest {
  schemaVersion: 1
  commandId: string
  args: unknown
}
export interface CommandReply {
  schemaVersion: 1
  result: unknown
  invalidateViews: string[]
}
export interface ServiceRequest {
  schemaVersion: 1
  service: string
  version: number
  method: string
  input: unknown
}
export interface ServiceReply {
  schemaVersion: 1
  result: unknown
}
export type UiGap = 'none' | 'xs' | 'sm' | 'md' | 'lg'
export type UiAlign = 'start' | 'center' | 'end' | 'stretch'
export type UiTone = 'default' | 'muted' | 'danger'
export type UiButtonVariant = 'default' | 'secondary' | 'outline' | 'destructive'
export interface SelectOption {
  value: string
  label: string
}
export interface ListItem {
  id: string
  text: string
  description?: string
}
export type UiNode = { id: string } & (
  | { type: 'column' | 'row'; props: { gap?: UiGap; align?: UiAlign }; children: UiNode[] }
  | { type: 'text'; props: { text: string; tone?: UiTone; wrap?: boolean } }
  | { type: 'heading'; props: { text: string; level?: 1 | 2 | 3 } }
  | {
      type: 'button'
      props: { label: string; actionId: string; variant?: UiButtonVariant; disabled?: boolean }
    }
  | {
      type: 'input'
      props: {
        label: string
        value: string
        actionId: string
        placeholder?: string
        maxLength?: number
        disabled?: boolean
      }
    }
  | {
      type: 'toggle'
      props: { label: string; checked: boolean; actionId: string; disabled?: boolean }
    }
  | {
      type: 'select'
      props: {
        label: string
        value: string
        actionId: string
        options: SelectOption[]
        disabled?: boolean
      }
    }
  | { type: 'list'; props: { items: ListItem[] } }
  | { type: 'divider'; props: { orientation?: 'horizontal' | 'vertical' } }
)
export interface UiTree {
  schemaVersion: 1
  viewId: string
  root: UiNode
}
export interface ActionEvent {
  schemaVersion: 1
  eventId: string
  pluginId: string
  generation: string
  viewId: string
  viewRevision: number
  nodeId: string
  actionId: string
  kind: 'click' | 'change'
  value: null | string | boolean
}
export interface ViewSnapshot {
  pluginId: string
  generation: string
  revision: number
  configRevision: number
  tree: UiTree
}
export type PluginState =
  | 'installed'
  | 'disabled'
  | 'pending'
  | 'activating'
  | 'active'
  | 'reloading'
  | 'failed'
  | 'quarantined'
export interface PluginError {
  code: string
  message: string
}
export interface PluginInfo {
  id: string
  manifest: Manifest
  desiredEnabled: boolean
  state: PluginState
  generation: null | string
  configRevision: number
  sha256: string
  lastError: null | PluginError
}
export interface PluginSnapshot {
  schemaVersion: 1
  bootId: string
  seq: number
  plugins: PluginInfo[]
}
export interface ConfigSnapshot {
  pluginId: string
  schema: ConfigurationSchema
  revision: number
  values: ConfigValues
  overrides: ConfigValues
}
export interface InstallRequest {
  path: string
}
export interface SetEnabledRequest {
  id: string
  enabled: boolean
}
export interface ReloadRequest {
  id: string
  path?: string
}
export interface UninstallRequest {
  id: string
}
export interface UninstallResult {
  id: string
  removed: true
}
export interface GetViewRequest {
  id: string
  viewId: string
}
export interface ExecuteCommandRequest {
  id: string
  generation: string
  commandId: string
  args: unknown
}
export interface GetConfigRequest {
  id: string
}
export interface SetConfigRequest {
  id: string
  expectedRevision: number
  values: ConfigValues
}
export type PluginEventType =
  | 'registry_changed'
  | 'state_changed'
  | 'view_changed'
  | 'config_changed'
  | 'resync_required'
export interface PluginEvent {
  schemaVersion: 1
  bootId: string
  seq: number
  type: PluginEventType
  pluginId?: string
  generation?: string
  viewId?: string
  configRevision?: number
}
export interface PluginConnectionChanged {
  state: 'connected' | 'disconnected'
  bootId: null | string
}
export interface PluginHealth {
  schemaVersion: 1
  bootId: string
  pid: number
  ready: boolean
  safeMode: boolean
  activeResources: number
}
export interface PluginApi {
  list(): Promise<IpcResult<PluginSnapshot>>
  install(request: InstallRequest): Promise<IpcResult<PluginInfo>>
  setEnabled(request: SetEnabledRequest): Promise<IpcResult<PluginInfo>>
  reload(request: ReloadRequest): Promise<IpcResult<PluginInfo>>
  uninstall(request: UninstallRequest): Promise<IpcResult<UninstallResult>>
  getView(request: GetViewRequest): Promise<IpcResult<ViewSnapshot>>
  dispatchAction(request: ActionEvent): Promise<IpcResult<ViewSnapshot>>
  executeCommand(request: ExecuteCommandRequest): Promise<IpcResult<CommandReply>>
  getConfig(request: GetConfigRequest): Promise<IpcResult<ConfigSnapshot>>
  setConfig(request: SetConfigRequest): Promise<IpcResult<ConfigSnapshot>>
  onChanged(handler: (event: PluginEvent) => void): () => void
  onConnectionChanged(handler: (event: PluginConnectionChanged) => void): () => void
}
const own = (o: object, k: string): boolean => Object.prototype.hasOwnProperty.call(o, k)
function bad(): never {
  throw new TypeError('Invalid plugin contract')
}
function rec(v: unknown): Record<string, unknown> {
  if (!v || typeof v !== 'object' || Array.isArray(v)) bad()
  return v as Record<string, unknown>
}
function obj(v: unknown, required: string[], optional: string[] = []): Record<string, unknown> {
  const o = rec(v)
  if (
    required.some((k) => !own(o, k)) ||
    Object.keys(o).some((k) => !required.includes(k) && !optional.includes(k))
  )
    bad()
  return o
}
function str(v: unknown, min = 0, max = 8192): string {
  if (typeof v !== 'string' || [...v].length < min || [...v].length > max) bad()
  return v
}
function int(v: unknown, min = 1, max = Number.MAX_SAFE_INTEGER): number {
  if (typeof v !== 'number' || !Number.isSafeInteger(v) || v < min || v > max) bad()
  return v
}
function bool(v: unknown): boolean {
  if (typeof v !== 'boolean') bad()
  return v
}
function one<T extends string>(v: unknown, choices: readonly T[]): T {
  if (typeof v !== 'string' || !choices.includes(v as T)) bad()
  return v as T
}
function arr(v: unknown, max: number): unknown[] {
  if (!Array.isArray(v) || v.length > max) bad()
  return v
}
function uniq(v: string[]): void {
  if (new Set(v).size !== v.length) bad()
}
function local(v: unknown): string {
  const s = str(v, 1, 128)
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(s)) bad()
  return s
}
function pid(v: unknown): string {
  const s = str(v, 1, 128)
  if (!/^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)+$/.test(s)) bad()
  return s
}
function uuid(v: unknown): string {
  const s = str(v, 36, 36)
  if (!/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(s)) bad()
  return s
}
function semver(v: unknown): string {
  const s = str(v, 1, Number.MAX_SAFE_INTEGER)
  const m =
    /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(
      s
    )
  if (
    !m ||
    m.slice(1, 4).some((n) => BigInt(n!) > 18446744073709551615n) ||
    m[4]?.split('.').some((n) => /^0[0-9]+$/.test(n))
  )
    bad()
  return s
}
function sized(v: unknown): void {
  const seen = new Set<object>()
  function json(value: unknown): void {
    if (value === null || typeof value === 'boolean' || typeof value === 'string') return
    if (typeof value === 'number') {
      if (!Number.isFinite(value)) bad()
      return
    }
    if (!value || typeof value !== 'object' || seen.has(value)) bad()
    const proto = Object.getPrototypeOf(value)
    if (!Array.isArray(value) && proto !== Object.prototype && proto !== null) bad()
    if (Object.getOwnPropertySymbols(value).length) bad()
    seen.add(value)
    if (Array.isArray(value)) {
      for (let i = 0; i < value.length; i++) {
        if (!own(value, String(i))) bad()
        json(value[i])
      }
    } else {
      for (const child of Object.values(value)) json(child)
    }
    seen.delete(value)
  }
  try {
    json(v)
  } catch {
    bad()
  }
  let s: string | undefined
  try {
    s = JSON.stringify(v)
  } catch {
    bad()
  }
  if (s === undefined || new TextEncoder().encode(s).length > PLUGIN_JSON_MAX_BYTES) bad()
}
function sv(v: unknown): 1 {
  if (v !== 1) bad()
  return 1
}
function scalar(v: unknown): ConfigValue {
  if (typeof v === 'string') return str(v)
  if (typeof v === 'boolean' || (typeof v === 'number' && Number.isFinite(v))) return v
  return bad()
}
function values(v: unknown): ConfigValues {
  const entries = Object.entries(rec(v))
  if (entries.length > 64) bad()
  return Object.fromEntries(entries.map(([k, x]) => [local(k), scalar(x)]))
}
function propertyValue(p: ConfigurationProperty, value: unknown): ConfigValue {
  const v = scalar(value)
  if (typeof v !== p.type) bad()
  if (typeof v === 'string') str(v, p.minLength ?? 0, p.maxLength ?? 8192)
  if (
    typeof v === 'number' &&
    ((p.minimum !== undefined && v < p.minimum) || (p.maximum !== undefined && v > p.maximum))
  )
    bad()
  if (p.enum && !p.enum.includes(v)) bad()
  return v
}
export function parseConfigurationSchema(v: unknown): ConfigurationSchema {
  sized(v)
  const o = obj(v, ['title', 'properties']),
    entries = Object.entries(rec(o.properties))
  if (entries.length > 64) bad()
  const properties = Object.fromEntries(
    entries.map(([k, x]) => {
      local(k)
      const p = obj(
        x,
        ['type', 'title', 'description', 'default'],
        ['enum', 'enumLabels', 'minLength', 'maxLength', 'minimum', 'maximum']
      )
      const type = one(p.type, ['string', 'number', 'boolean'])
      const r: ConfigurationProperty = {
        type,
        title: str(p.title, 1, 128),
        description: str(p.description, 0, 2048),
        default: scalar(p.default)
      }
      for (const key of ['minLength', 'maxLength'] as const)
        if (own(p, key)) {
          if (type !== 'string') bad()
          r[key] = int(p[key], 0, 8192)
        }
      if ((r.minLength ?? 0) > (r.maxLength ?? 8192)) bad()
      for (const key of ['minimum', 'maximum'] as const)
        if (own(p, key)) {
          if (type !== 'number' || typeof p[key] !== 'number' || !Number.isFinite(p[key])) bad()
          r[key] = p[key] as number
        }
      if (r.minimum !== undefined && r.maximum !== undefined && r.minimum > r.maximum) bad()
      if (own(p, 'enum')) {
        r.enum = arr(p.enum, 64).map(scalar)
        if (!r.enum.length || new Set(r.enum).size !== r.enum.length) bad()
        r.enum.forEach((x) => {
          if (typeof x !== type) bad()
        })
      }
      if (own(p, 'enumLabels')) {
        r.enumLabels = arr(p.enumLabels, 64).map((x) => str(x))
        if (!r.enum || r.enumLabels.length !== r.enum.length) bad()
      }
      propertyValue(r, r.default)
      return [k, r]
    })
  )
  return { title: str(o.title, 1, 128), properties }
}
export function validateConfigValues(
  schema: ConfigurationSchema,
  input: unknown,
  complete = false
): ConfigValues {
  const v = rec(input)
  if (Object.keys(v).some((k) => !own(schema.properties, k))) bad()
  return Object.fromEntries(
    Object.entries(schema.properties).map(([k, p]) => {
      if (complete && !own(v, k)) bad()
      return [k, propertyValue(p, own(v, k) ? v[k] : p.default)]
    })
  )
}
export function parseManifest(v: unknown): Manifest {
  sized(v)
  const o = obj(v, ['schemaVersion', 'id', 'name', 'version', 'abi', 'contributes', 'services']),
    a = obj(o.abi, ['min', 'max'])
  const min = int(a.min, 1, 0xffffffff),
    max = int(a.max, 1, 0xffffffff)
  if (min > max) bad()
  const c = obj(o.contributes, ['commands', 'configuration', 'views'])
  const commands = arr(c.commands, 64).map((v) => {
    const x = obj(v, ['id', 'title'], ['description'])
    return {
      id: local(x.id),
      title: str(x.title, 1, 128),
      ...(own(x, 'description') ? { description: str(x.description, 0, 2048) } : {})
    }
  })
  uniq(commands.map((x) => x.id))
  const views = arr(c.views, 8).map((v) => {
    const x = obj(v, ['id', 'title', 'location'], ['icon'])
    return {
      id: local(x.id),
      title: str(x.title, 1, 128),
      location: one(x.location, ['panel', 'sidebar']),
      ...(own(x, 'icon') ? { icon: one(x.icon, viewIcons) } : {})
    }
  })
  uniq(views.map((x) => x.id))
  if (views.filter((view) => view.location === 'sidebar').length > 2) bad()
  const s = obj(o.services, ['provides', 'requires'])
  const services = (v: unknown): PluginService[] => {
    const r = arr(v, 32).map((v) => {
      const x = obj(v, ['name', 'version'])
      return { name: pid(x.name), version: int(x.version, 1, 0x7fffffff) }
    })
    uniq(r.map((x) => `${x.name}:${x.version}`))
    return r
  }
  return {
    schemaVersion: sv(o.schemaVersion),
    id: pid(o.id),
    name: str(o.name, 1, 128),
    version: semver(o.version),
    abi: { min, max },
    contributes: { commands, views, configuration: parseConfigurationSchema(c.configuration) },
    services: { provides: services(s.provides), requires: services(s.requires) }
  }
}
export const parsePluginManifest = parseManifest

export function parseActivationPayload(
  v: unknown,
  schema?: ConfigurationSchema
): ActivationPayload {
  sized(v)
  const o = obj(v, ['schemaVersion', 'pluginId', 'generation', 'config'])
  const pluginId = pid(o.pluginId),
    config = parseConfigPayload(o.config, schema)
  if (config.pluginId !== pluginId || config.changedKeys.length) bad()
  return { schemaVersion: sv(o.schemaVersion), pluginId, generation: uuid(o.generation), config }
}
export function parseRenderRequest(v: unknown): RenderRequest {
  const o = obj(v, ['schemaVersion', 'viewId'])
  return { schemaVersion: sv(o.schemaVersion), viewId: local(o.viewId) }
}
export function parseCommandRequest(v: unknown): CommandRequest {
  sized(v)
  const o = obj(v, ['schemaVersion', 'commandId', 'args'])
  return { schemaVersion: sv(o.schemaVersion), commandId: local(o.commandId), args: o.args }
}
export function parseServiceRequest(v: unknown): ServiceRequest {
  sized(v)
  const o = obj(v, ['schemaVersion', 'service', 'version', 'method', 'input'])
  return {
    schemaVersion: sv(o.schemaVersion),
    service: pid(o.service),
    version: int(o.version, 1, 0x7fffffff),
    method: str(o.method),
    input: o.input
  }
}
export function parseServiceReply(v: unknown): ServiceReply {
  sized(v)
  const o = obj(v, ['schemaVersion', 'result'])
  return { schemaVersion: sv(o.schemaVersion), result: o.result }
}
export function parseInstallRequest(v: unknown): InstallRequest {
  const o = obj(v, ['path'])
  return { path: str(o.path, 1) }
}
export function parseSetEnabledRequest(v: unknown): SetEnabledRequest {
  const o = obj(v, ['id', 'enabled'])
  return { id: pid(o.id), enabled: bool(o.enabled) }
}
export function parseReloadRequest(v: unknown): ReloadRequest {
  const o = obj(v, ['id'], ['path'])
  return { id: pid(o.id), ...(own(o, 'path') ? { path: str(o.path, 1) } : {}) }
}
export function parseGetConfigRequest(v: unknown): GetConfigRequest {
  const o = obj(v, ['id'])
  return { id: pid(o.id) }
}
export const parseUninstallRequest = parseGetConfigRequest
export function parseGetViewRequest(v: unknown): GetViewRequest {
  const o = obj(v, ['id', 'viewId'])
  return { id: pid(o.id), viewId: local(o.viewId) }
}
export function parseExecuteCommandRequest(v: unknown): ExecuteCommandRequest {
  sized(v)
  const o = obj(v, ['id', 'generation', 'commandId', 'args'])
  return {
    id: pid(o.id),
    generation: uuid(o.generation),
    commandId: local(o.commandId),
    args: o.args
  }
}
export function parseSetConfigRequest(v: unknown): SetConfigRequest {
  sized(v)
  const o = obj(v, ['id', 'expectedRevision', 'values'])
  return { id: pid(o.id), expectedRevision: int(o.expectedRevision), values: values(o.values) }
}
export function parsePluginHealth(v: unknown): PluginHealth {
  const o = obj(v, ['schemaVersion', 'bootId', 'pid', 'ready', 'safeMode', 'activeResources'])
  return {
    schemaVersion: sv(o.schemaVersion),
    bootId: uuid(o.bootId),
    pid: int(o.pid, 1, 0xffffffff),
    ready: bool(o.ready),
    safeMode: bool(o.safeMode),
    activeResources: int(o.activeResources, 0)
  }
}
export function parseConfigPayload(v: unknown, schema?: ConfigurationSchema): ConfigPayload {
  sized(v)
  const o = obj(v, ['schemaVersion', 'pluginId', 'revision', 'values', 'changedKeys'])
  const changedKeys = arr(o.changedKeys, 64).map(local)
  uniq(changedKeys)
  const val = schema ? validateConfigValues(schema, o.values, true) : values(o.values)
  if (changedKeys.some((k) => !own(val, k))) bad()
  return {
    schemaVersion: sv(o.schemaVersion),
    pluginId: pid(o.pluginId),
    revision: int(o.revision),
    values: val,
    changedKeys
  }
}
export function parseUiTree(v: unknown): UiTree {
  sized(v)
  const o = obj(v, ['schemaVersion', 'viewId', 'root'])
  const ids = new Set<string>()
  let count = 0
  function node(v: unknown, depth: number): UiNode {
    if (depth > 32 || ++count > 1024) bad()
    const raw = rec(v)
    const container = raw.type === 'row' || raw.type === 'column'
    const n = obj(v, container ? ['id', 'type', 'props', 'children'] : ['id', 'type', 'props'])
    const id = local(n.id)
    if (ids.has(id)) bad()
    ids.add(id)
    const type = one(n.type, [
      'column',
      'row',
      'text',
      'heading',
      'button',
      'input',
      'toggle',
      'select',
      'list',
      'divider'
    ])
    let p: Record<string, unknown>
    let props: Record<string, unknown>
    switch (type) {
      case 'column':
      case 'row':
        p = obj(n.props, [], ['gap', 'align'])
        props = {
          gap: own(p, 'gap') ? one(p.gap, ['none', 'xs', 'sm', 'md', 'lg']) : 'sm',
          align: own(p, 'align') ? one(p.align, ['start', 'center', 'end', 'stretch']) : 'stretch'
        }
        return {
          id,
          type,
          props,
          children: arr(n.children, 1024).map((x) => node(x, depth + 1))
        } as UiNode
      case 'text':
        p = obj(n.props, ['text'], ['tone', 'wrap'])
        props = {
          text: str(p.text),
          tone: own(p, 'tone') ? one(p.tone, ['default', 'muted', 'danger']) : 'default',
          wrap: own(p, 'wrap') ? bool(p.wrap) : true
        }
        break
      case 'heading':
        p = obj(n.props, ['text'], ['level'])
        props = { text: str(p.text), level: own(p, 'level') ? int(p.level, 1, 3) : 2 }
        break
      case 'button':
        p = obj(n.props, ['label', 'actionId'], ['variant', 'disabled'])
        props = {
          label: str(p.label, 1, 128),
          actionId: local(p.actionId),
          variant: own(p, 'variant')
            ? one(p.variant, ['default', 'secondary', 'outline', 'destructive'])
            : 'default',
          disabled: own(p, 'disabled') ? bool(p.disabled) : false
        }
        break
      case 'input':
        p = obj(n.props, ['label', 'value', 'actionId'], ['placeholder', 'maxLength', 'disabled'])
        {
          const maxLength = own(p, 'maxLength') ? int(p.maxLength, 1, 4096) : 4096
          props = {
            label: str(p.label, 1, 128),
            actionId: local(p.actionId),
            value: str(p.value, 0, maxLength),
            placeholder: own(p, 'placeholder') ? str(p.placeholder) : '',
            maxLength,
            disabled: own(p, 'disabled') ? bool(p.disabled) : false
          }
        }
        break
      case 'toggle':
        p = obj(n.props, ['label', 'checked', 'actionId'], ['disabled'])
        props = {
          label: str(p.label, 1, 128),
          checked: bool(p.checked),
          actionId: local(p.actionId),
          disabled: own(p, 'disabled') ? bool(p.disabled) : false
        }
        break
      case 'select':
        p = obj(n.props, ['label', 'value', 'actionId', 'options'], ['disabled'])
        {
          const options = arr(p.options, 64).map((v) => {
            const x = obj(v, ['value', 'label'])
            return { value: str(x.value, 1), label: str(x.label) }
          })
          uniq(options.map((x) => x.value))
          const value = str(p.value)
          if (!options.some((x) => x.value === value)) bad()
          props = {
            label: str(p.label, 1, 128),
            value,
            actionId: local(p.actionId),
            options,
            disabled: own(p, 'disabled') ? bool(p.disabled) : false
          }
        }
        break
      case 'list':
        p = obj(n.props, ['items'])
        {
          const items = arr(p.items, 256).map((v) => {
            const x = obj(v, ['id', 'text'], ['description'])
            return {
              id: str(x.id),
              text: str(x.text),
              description: own(x, 'description') ? str(x.description) : ''
            }
          })
          uniq(items.map((x) => x.id))
          props = { items }
        }
        break
      case 'divider':
        p = obj(n.props, [], ['orientation'])
        props = {
          orientation: own(p, 'orientation')
            ? one(p.orientation, ['horizontal', 'vertical'])
            : 'horizontal'
        }
        break
    }
    return { id, type, props } as UiNode
  }
  return { schemaVersion: sv(o.schemaVersion), viewId: local(o.viewId), root: node(o.root, 1) }
}
export function parseActionEvent(v: unknown): ActionEvent {
  sized(v)
  const o = obj(v, [
    'schemaVersion',
    'eventId',
    'pluginId',
    'generation',
    'viewId',
    'viewRevision',
    'nodeId',
    'actionId',
    'kind',
    'value'
  ])
  const kind = one(o.kind, ['click', 'change'])
  if (
    kind === 'click'
      ? o.value !== null
      : typeof o.value !== 'string' && typeof o.value !== 'boolean'
  )
    bad()
  return {
    schemaVersion: sv(o.schemaVersion),
    eventId: uuid(o.eventId),
    pluginId: pid(o.pluginId),
    generation: uuid(o.generation),
    viewId: local(o.viewId),
    viewRevision: int(o.viewRevision),
    nodeId: local(o.nodeId),
    actionId: local(o.actionId),
    kind,
    value: typeof o.value === 'string' ? str(o.value, 0, 8192) : (o.value as null | boolean)
  }
}
export function validateActionForView(input: unknown, view: ViewSnapshot): ActionEvent {
  const a = parseActionEvent(input)
  if (
    a.pluginId !== view.pluginId ||
    a.generation !== view.generation ||
    a.viewId !== view.tree.viewId ||
    a.viewRevision !== view.revision
  )
    bad()
  const find = (n: UiNode): UiNode | undefined =>
    n.id === a.nodeId ? n : 'children' in n ? n.children.map(find).find(Boolean) : undefined
  const n = find(view.tree.root)
  if (!n || !('actionId' in n.props) || n.props.actionId !== a.actionId || n.props.disabled) bad()
  if (n.type === 'button') {
    if (a.kind !== 'click' || a.value !== null) bad()
  } else if (n.type === 'toggle') {
    if (a.kind !== 'change' || typeof a.value !== 'boolean') bad()
  } else if (n.type === 'input') {
    if (a.kind !== 'change') bad()
    str(a.value, 0, n.props.maxLength ?? 4096)
  } else if (n.type === 'select') {
    if (a.kind !== 'change' || !n.props.options.some((x) => x.value === a.value)) bad()
  } else bad()
  return a
}
export function parseViewSnapshot(v: unknown): ViewSnapshot {
  const o = obj(v, ['pluginId', 'generation', 'revision', 'configRevision', 'tree'])
  return {
    pluginId: pid(o.pluginId),
    generation: uuid(o.generation),
    revision: int(o.revision),
    configRevision: int(o.configRevision),
    tree: parseUiTree(o.tree)
  }
}
export function parsePluginError(v: unknown): PluginError {
  const o = obj(v, ['code', 'message'])
  const code = str(o.code, 1)
  if (!/^PLUGIN_[A-Z_]+$/.test(code)) bad()
  return { code, message: str(o.message) }
}
export function parsePluginInfo(v: unknown): PluginInfo {
  const o = obj(v, [
    'id',
    'manifest',
    'desiredEnabled',
    'state',
    'generation',
    'configRevision',
    'sha256',
    'lastError'
  ])
  const id = pid(o.id),
    manifest = parseManifest(o.manifest)
  if (id !== manifest.id) bad()
  const sha256 = str(o.sha256, 64, 64)
  if (!/^[0-9a-f]{64}$/i.test(sha256)) bad()
  return {
    id,
    manifest,
    desiredEnabled: bool(o.desiredEnabled),
    state: one(o.state, [
      'installed',
      'disabled',
      'pending',
      'activating',
      'active',
      'reloading',
      'failed',
      'quarantined'
    ]),
    generation: o.generation === null ? null : uuid(o.generation),
    configRevision: int(o.configRevision),
    sha256,
    lastError: o.lastError === null ? null : parsePluginError(o.lastError)
  }
}
export function parsePluginSnapshot(v: unknown): PluginSnapshot {
  const o = obj(v, ['schemaVersion', 'bootId', 'seq', 'plugins'])
  const plugins = arr(o.plugins, Number.MAX_SAFE_INTEGER).map(parsePluginInfo)
  uniq(plugins.map((x) => x.id))
  return { schemaVersion: sv(o.schemaVersion), bootId: uuid(o.bootId), seq: int(o.seq, 0), plugins }
}
export function parseConfigSnapshot(v: unknown): ConfigSnapshot {
  sized(v)
  const o = obj(v, ['pluginId', 'schema', 'revision', 'values', 'overrides'])
  const schema = parseConfigurationSchema(o.schema),
    overrides = values(o.overrides)
  validateConfigValues(schema, overrides)
  const val = validateConfigValues(schema, o.values, true)
  const effective = validateConfigValues(schema, overrides)
  if (Object.keys(val).some((k) => val[k] !== effective[k])) bad()
  return { pluginId: pid(o.pluginId), schema, revision: int(o.revision), values: val, overrides }
}
export function parseCommandReply(v: unknown): CommandReply {
  sized(v)
  const o = obj(v, ['schemaVersion', 'result', 'invalidateViews'])
  const invalidateViews = arr(o.invalidateViews, 8).map(local)
  uniq(invalidateViews)
  return { schemaVersion: sv(o.schemaVersion), result: o.result, invalidateViews }
}
export function parseUninstallResult(v: unknown): UninstallResult {
  const o = obj(v, ['id', 'removed'])
  if (o.removed !== true) bad()
  return { id: pid(o.id), removed: true }
}
export function parsePluginEvent(v: unknown): PluginEvent {
  const o = obj(
    v,
    ['schemaVersion', 'bootId', 'seq', 'type'],
    ['pluginId', 'generation', 'viewId', 'configRevision']
  )
  return {
    schemaVersion: sv(o.schemaVersion),
    bootId: uuid(o.bootId),
    seq: int(o.seq, 0),
    type: one(o.type, [
      'registry_changed',
      'state_changed',
      'view_changed',
      'config_changed',
      'resync_required'
    ]),
    ...(own(o, 'pluginId') ? { pluginId: pid(o.pluginId) } : {}),
    ...(own(o, 'generation') ? { generation: uuid(o.generation) } : {}),
    ...(own(o, 'viewId') ? { viewId: local(o.viewId) } : {}),
    ...(own(o, 'configRevision') ? { configRevision: int(o.configRevision) } : {})
  }
}
export function parsePluginConnectionChanged(v: unknown): PluginConnectionChanged {
  const o = obj(v, ['state', 'bootId'])
  return {
    state: one(o.state, ['connected', 'disconnected']),
    bootId: o.bootId === null ? null : uuid(o.bootId)
  }
}
