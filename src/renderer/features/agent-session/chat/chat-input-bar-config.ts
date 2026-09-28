/**
 * Pure helpers for the input-bar config-option chip row. Kept free of
 * React/store so they can be unit-tested directly. Partitions agent-advertised
 * config options so the `model` and `thought_level` controls can be promoted
 * to dedicated chips rendered ahead of generic options
 * (issue #286).
 */
import type {
  SessionConfigOption,
  SessionConfigOptionValue,
  SessionModelState,
  SessionModeState
} from '@/lib/acp-api'

/** ACP semantic category for reasoning/thinking-depth config options. */
export const THOUGHT_LEVEL_CATEGORY = 'thought_level'
/** ACP semantic category for model selection config options. */
export const MODEL_CATEGORY = 'model'
/** ACP semantic category for session mode config options. */
export const MODE_CATEGORY = 'mode'

export interface PartitionedConfigOptions {
  /** The first `model` option, if the agent advertises one. */
  model: SessionConfigOption | null
  /** The first `thought_level` option, if the agent advertises one. */
  thoughtLevel: SessionConfigOption | null
  /** All remaining options, in their original relative order. */
  rest: SessionConfigOption[]
}

export interface ResolvedModelOption {
  option: SessionConfigOption | null
  source: 'config' | 'models' | null
}

type WireSelectGroup = {
  group?: string
  name?: string
  options: unknown[]
}

function isSelectValue(entry: unknown): entry is SessionConfigOptionValue {
  if (typeof entry !== 'object' || entry === null) return false
  const rec = entry as SessionConfigOptionValue
  return typeof rec.value === 'string' && typeof rec.name === 'string'
}

function isSelectGroup(entry: unknown): entry is WireSelectGroup {
  if (typeof entry !== 'object' || entry === null) return false
  const rec = entry as { options?: unknown }
  return Array.isArray(rec.options) && rec.options.length > 0
}

/**
 * Claude ACP groups families (`claude-sonnet`) above versioned leaves
 * (`claude-sonnet-5[1m]`). A family-only id is not a valid session value.
 */
export function canonicalizeClaudeModelId(value: string): string {
  const match = /^(claude-(?:sonnet|opus|haiku))(\[[^\]]+])?$/.exec(value)
  if (!match) return value
  return `${match[1]}-5${match[2] ?? ''}`
}

function joinSuffixValue(ancestor: string | undefined, value: string): string {
  if (ancestor && /^\[[^\]]+]$/.test(value)) return `${ancestor}${value}`
  return value
}

/**
 * ACP select options may be a flat `{value,name}[]` or grouped by provider
 * (`{group,name,options:[]}[]`, e.g. Claude). Families may also carry a
 * `value` plus nested leaves; recurse whenever `options` is present so the
 * picker never sends a parent family id. Suffix-only children (`[1m]`) join
 * the nearest ancestor value.
 */
export function flattenConfigOptionValues(
  options: SessionConfigOption['options'] | null | undefined,
  ancestorValue?: string
): SessionConfigOptionValue[] {
  if (!Array.isArray(options)) return []
  const flat: SessionConfigOptionValue[] = []
  for (const entry of options) {
    if (typeof entry !== 'object' || entry === null) continue
    const rec = entry as SessionConfigOptionValue & WireSelectGroup & { value?: string }
    const parentValue = typeof rec.value === 'string' ? rec.value : ancestorValue
    const groupName = rec.name || rec.group || ''
    if (isSelectGroup(rec)) {
      const children = flattenConfigOptionValues(
        rec.options as SessionConfigOption['options'],
        parentValue
      )
      if (children.length > 0) {
        for (const child of children) {
          flat.push({
            ...child,
            ...(groupName && !child.group ? { group: groupName } : {})
          })
        }
        continue
      }
    }
    if (isSelectValue(rec)) {
      flat.push({
        value: canonicalizeClaudeModelId(joinSuffixValue(ancestorValue, rec.value)),
        name: rec.name,
        ...(rec.description != null ? { description: rec.description } : {}),
        ...(typeof rec.group === 'string' && rec.group ? { group: rec.group } : {})
      })
    }
  }
  return flat
}

/** Flatten grouped select values and drop unusable currentValue shapes. */
export function normalizeSessionConfigOption(option: SessionConfigOption): SessionConfigOption {
  const raw = option.options
  const options = flattenConfigOptionValues(raw)
  const currentValue = canonicalizeClaudeModelId(
    typeof option.currentValue === 'string' ? option.currentValue : ''
  )
  if (
    Array.isArray(raw) &&
    raw.every(isSelectValue) &&
    currentValue === option.currentValue &&
    options.length === raw.length &&
    options.every(
      (entry, index) => entry.value === raw[index]?.value && entry.name === raw[index]?.name
    )
  ) {
    return option
  }
  return {
    ...option,
    currentValue,
    options
  }
}

/**
 * Split usable config options into promoted `model` / `thought_level` options
 * (first match wins for each) and the rest, preserving the rest's original
 * order. Options with an unknown/other category fall through to `rest` and
 * render as plain chips.
 */
export function partitionConfigOptions(options: SessionConfigOption[]): PartitionedConfigOptions {
  let model: SessionConfigOption | null = null
  let thoughtLevel: SessionConfigOption | null = null
  const rest: SessionConfigOption[] = []
  for (const raw of options) {
    const option = normalizeSessionConfigOption(raw)
    if (option.options.length === 0) continue
    if (model === null && option.category === MODEL_CATEGORY) {
      model = option
    } else if (thoughtLevel === null && option.category === THOUGHT_LEVEL_CATEGORY) {
      thoughtLevel = option
    } else {
      rest.push(option)
    }
  }
  return { model, thoughtLevel, rest }
}

/**
 * ACP has two model-selection shapes in the wild: generic config options and
 * the native session model state. Prefer config options when present, then
 * synthesize a picker-compatible option from `session.models`.
 */
export function resolveModelOption(
  configModel: SessionConfigOption | null,
  models: SessionModelState | null | undefined
): ResolvedModelOption {
  if (configModel) {
    const option = normalizeSessionConfigOption(configModel)
    if (option.options.length > 0) return { option, source: 'config' }
  }
  if (!models || models.availableModels.length === 0) return { option: null, source: null }
  return {
    source: 'models',
    option: {
      id: MODEL_CATEGORY,
      name: 'Model',
      category: MODEL_CATEGORY,
      type: 'select',
      currentValue: canonicalizeClaudeModelId(models.currentModelId),
      options: models.availableModels.map((model) => ({
        value: canonicalizeClaudeModelId(model.modelId),
        name: model.name,
        description: model.description ?? undefined
      }))
    }
  }
}

/**
 * Some agents advertise modes both through `session.modes` and a `mode` config
 * option. When the native modes API is available, keep one Agent picker that
 * calls `session/set_mode` instead of rendering a duplicate config chip.
 */
export function filterDuplicateModeConfigOptions(
  options: SessionConfigOption[],
  modes: SessionModeState | null
): SessionConfigOption[] {
  if (!modes || modes.availableModes.length === 0) return options
  return options.filter((option) => option.category !== MODE_CATEGORY)
}

const ON_TOKEN = /^(on|true|enabled|1)$/i
const OFF_TOKEN = /^(off|false|disabled|0)$/i

function isOnOffToken(value: string): boolean {
  return ON_TOKEN.test(value) || OFF_TOKEN.test(value)
}

/** True when an option is a two-value On/Off (or true/false) switch. */
export function isBinaryOnOffOption(option: SessionConfigOption): boolean {
  if (option.options.length !== 2) return false
  return option.options.every((entry) => isOnOffToken(entry.value) || isOnOffToken(entry.name))
}

/**
 * Detect the Cursor-style Fast Mode switch advertised as a generic select with
 * On/Off values. Matched by id/name/category containing "fast".
 */
export function isFastModeOption(option: SessionConfigOption): boolean {
  const haystack = `${option.id} ${option.name} ${option.category ?? ''}`.toLowerCase()
  return haystack.includes('fast') && isBinaryOnOffOption(option)
}

function entryIsOn(entry: SessionConfigOption['options'][number]): boolean {
  return ON_TOKEN.test(entry.value) || ON_TOKEN.test(entry.name)
}

/** Whether the option's current value is the On side of a Fast Mode switch. */
export function isFastModeEnabled(
  option: SessionConfigOption,
  currentValue: string = option.currentValue
): boolean {
  const current = option.options.find((entry) => entry.value === currentValue)
  if (!current) return false
  return entryIsOn(current)
}

/** Opposite value for a Fast Mode toggle click. */
export function oppositeFastModeValue(
  option: SessionConfigOption,
  currentValue: string = option.currentValue
): string | null {
  const other = option.options.find((entry) => entry.value !== currentValue)
  return other?.value ?? null
}

/**
 * Pull the first Fast Mode switch out of a generic options list so it can
 * render as an icon toggle instead of a labeled select pill.
 */
export function extractFastModeOption(options: SessionConfigOption[]): {
  fastMode: SessionConfigOption | null
  rest: SessionConfigOption[]
} {
  const index = options.findIndex(isFastModeOption)
  if (index < 0) return { fastMode: null, rest: options }
  const fastMode = options[index] ?? null
  const rest = options.filter((_, i) => i !== index)
  return { fastMode, rest }
}
