/**
 * Persistent ActivityRail layout.
 *
 * The brand mark is not an entry and cannot be dragged. Dividers are
 * structural markers: unknown or duplicate entries are dropped on load, and
 * missing default entries are inserted beside the nearest preceding default
 * neighbor. Users may add their own `sep-N` dividers; only those can be
 * removed, because the built-in dividers are defaults and always return.
 * Utility actions stay pinned unless a saved layout opts them into the
 * sortable region by including them.
 */

import {
  CONTRACT_MAX_SAFE_INTEGER,
  contractFail,
  isForbiddenCredentialKey,
  isRecord,
  readClosedObject,
  requireInteger
} from './contract-guards'

export const RAIL_LAYOUT_SCHEMA_VERSION = 1 as const
export const RAIL_ENTRIES_MAX = 64
export const RAIL_BRAND_ENTRY_ID = 'brand'

export const RAIL_SORTABLE_ITEM_IDS = [
  'projects',
  'terminals',
  'gitChanges',
  'gitHistory',
  'ssh',
  'conversations',
  'skills',
  'aiChannels',
  'mcp',
  'scheduledTasks'
] as const

export const RAIL_UTILITY_ITEM_IDS = ['shortcuts', 'preferences', 'themes'] as const

export const RAIL_DIVIDER_IDS = ['workspace-contexts', 'tools'] as const

export const RAIL_ITEM_IDS = [...RAIL_SORTABLE_ITEM_IDS, ...RAIL_UTILITY_ITEM_IDS] as const

export type RailSortableItemId = (typeof RAIL_SORTABLE_ITEM_IDS)[number]
export type RailUtilityItemId = (typeof RAIL_UTILITY_ITEM_IDS)[number]
export type RailBuiltinDividerId = (typeof RAIL_DIVIDER_IDS)[number]
export type RailUserDividerId = `sep-${number}`
export type RailDividerId = RailBuiltinDividerId | RailUserDividerId
export type RailItemId = (typeof RAIL_ITEM_IDS)[number]
export type RailItemMobility = 'sortable' | 'utility'

export type RailLayoutEntry =
  | { kind: 'item'; id: RailItemId }
  | { kind: 'divider'; id: RailDividerId }

export interface RailLayoutConfig {
  schemaVersion: typeof RAIL_LAYOUT_SCHEMA_VERSION
  revision: number
  entries: RailLayoutEntry[]
}

export interface RailRevisionFence {
  lastAccepted: number | null
}

const INVALID = 'navigation layout is invalid'
const FORBIDDEN = 'navigation layout contains a forbidden credential field'

const DEFAULT_ENTRIES: readonly RailLayoutEntry[] = [
  { kind: 'item', id: 'projects' },
  { kind: 'item', id: 'terminals' },
  { kind: 'item', id: 'gitChanges' },
  { kind: 'item', id: 'gitHistory' },
  { kind: 'item', id: 'ssh' },
  { kind: 'divider', id: 'workspace-contexts' },
  { kind: 'item', id: 'conversations' },
  { kind: 'divider', id: 'tools' },
  { kind: 'item', id: 'skills' },
  { kind: 'item', id: 'aiChannels' },
  { kind: 'item', id: 'mcp' },
  { kind: 'item', id: 'scheduledTasks' }
]

export function defaultRailLayout(): RailLayoutConfig {
  return {
    schemaVersion: RAIL_LAYOUT_SCHEMA_VERSION,
    revision: 0,
    entries: DEFAULT_ENTRIES.map(cloneEntry)
  }
}

export function railItemMobility(id: RailItemId): RailItemMobility {
  return isUtilityItemId(id) ? 'utility' : 'sortable'
}

export function normalizeRailLayout(value: unknown): RailLayoutConfig {
  if (value === undefined || value === null) return defaultRailLayout()
  const record = readClosedObject(
    value,
    ['schemaVersion', 'revision', 'entries'],
    INVALID,
    FORBIDDEN
  )
  if (record.schemaVersion !== RAIL_LAYOUT_SCHEMA_VERSION) contractFail(INVALID)
  const revision = requireInteger(record.revision, 0, CONTRACT_MAX_SAFE_INTEGER, INVALID)
  if (!Array.isArray(record.entries) || record.entries.length > RAIL_ENTRIES_MAX) {
    contractFail(INVALID)
  }

  const entries: RailLayoutEntry[] = []
  const seen = new Set<string>()
  for (const candidate of record.entries) {
    const parsed = parseEntry(candidate)
    if (!parsed) continue
    const key = entryKey(parsed)
    if (seen.has(key)) continue
    seen.add(key)
    entries.push(parsed)
  }

  for (const expected of DEFAULT_ENTRIES) {
    const key = entryKey(expected)
    if (seen.has(key)) continue
    insertAfterNearestPredecessor(entries, expected)
    seen.add(key)
  }

  return { schemaVersion: RAIL_LAYOUT_SCHEMA_VERSION, revision, entries }
}

export function createRailRevisionFence(): RailRevisionFence {
  return { lastAccepted: null }
}

export function acceptRailRevision(fence: RailRevisionFence, revision: number): void {
  if (!Number.isInteger(revision) || revision < 1 || revision > CONTRACT_MAX_SAFE_INTEGER) {
    contractFail('navigation revision is invalid')
  }
  if (fence.lastAccepted !== null && revision <= fence.lastAccepted) {
    contractFail('navigation revision is stale')
  }
  fence.lastAccepted = revision
}

export type RailReorderPlacement =
  | { kind: 'to-index'; index: number }
  | { kind: 'by'; delta: -1 | 1 }
  | { kind: 'pin' }

export type RailReorderFeedback =
  | { kind: 'unchanged'; reason: 'first' | 'last' | 'same'; id: RailItemId }
  | {
      kind: 'moved-before'
      id: RailItemId
      neighborKind: 'item' | 'divider'
      neighborId: RailItemId | RailDividerId
    }
  | {
      kind: 'moved-after'
      id: RailItemId
      neighborKind: 'item' | 'divider'
      neighborId: RailItemId | RailDividerId
    }
  | { kind: 'pinned'; id: RailUtilityItemId }

export interface RailReorderResult {
  changed: boolean
  entries: RailLayoutEntry[]
  feedback: RailReorderFeedback
}

/**
 * Move one rail item. Dividers are not movable subjects. Sortable items stay
 * on the rail. A utility item is pinned (absent) until a move places it in
 * `entries`; moving it past the end pins it again.
 *
 * `to-index` is the index in the list after the moving item is removed, or in
 * the full list when the utility is not yet present.
 */
export function applyRailReorder(
  entries: readonly RailLayoutEntry[],
  id: RailItemId,
  placement: RailReorderPlacement
): RailReorderResult {
  if (!isRailItemId(id)) {
    return unchanged(entries, id, 'same')
  }
  if (placement.kind === 'pin') return pinUtility(entries, id)
  if (placement.kind === 'by') return moveBy(entries, id, placement.delta)
  if (!Number.isInteger(placement.index)) return unchanged(entries, id, 'same')
  return moveToRestIndex(entries, id, placement.index)
}

/** Insert a new user divider before `beforeEntryIndex` (clamped to the list). */
export function insertRailDivider(
  entries: readonly RailLayoutEntry[],
  beforeEntryIndex: number
): RailLayoutEntry[] {
  const next = entries.map(cloneEntry)
  if (next.length >= RAIL_ENTRIES_MAX) return next
  const index = Math.min(next.length, Math.max(0, Math.trunc(beforeEntryIndex)))
  next.splice(index, 0, { kind: 'divider', id: nextUserDividerId(entries) })
  return next
}

/** Remove a user divider. Built-in dividers are defaults and are kept. */
export function removeRailDivider(
  entries: readonly RailLayoutEntry[],
  id: RailDividerId
): RailLayoutEntry[] {
  return entries
    .filter((entry) => !(entry.kind === 'divider' && entry.id === id && isUserDividerId(id)))
    .map(cloneEntry)
}

export function isUserDividerId(value: string): value is RailUserDividerId {
  return USER_DIVIDER_ID.test(value)
}

export function sameRailEntries(
  left: readonly RailLayoutEntry[],
  right: readonly RailLayoutEntry[]
): boolean {
  if (left.length !== right.length) return false
  return left.every(
    (entry, index) => entry.kind === right[index]?.kind && entry.id === right[index]?.id
  )
}

function pinUtility(entries: readonly RailLayoutEntry[], id: RailItemId): RailReorderResult {
  if (!isUtilityItemId(id)) return unchanged(entries, id, 'same')
  const index = itemIndex(entries, id)
  if (index < 0) return unchanged(entries, id, 'same')
  return {
    changed: true,
    entries: entries.filter((_, entryIndex) => entryIndex !== index).map(cloneEntry),
    feedback: { kind: 'pinned', id }
  }
}

function moveBy(
  entries: readonly RailLayoutEntry[],
  id: RailItemId,
  delta: -1 | 1
): RailReorderResult {
  const index = itemIndex(entries, id)
  if (index < 0) {
    if (!isUtilityItemId(id) || delta > 0) {
      return unchanged(entries, id, delta > 0 ? 'last' : 'same')
    }
    const next = entries.map(cloneEntry)
    next.push({ kind: 'item', id })
    return {
      changed: true,
      entries: next,
      feedback: describeMove(entries, next, id)
    }
  }
  const target = index + delta
  if (target < 0) return unchanged(entries, id, 'first')
  if (target >= entries.length) {
    if (isUtilityItemId(id)) return pinUtility(entries, id)
    return unchanged(entries, id, 'last')
  }
  const next = entries.map(cloneEntry)
  const [item] = next.splice(index, 1)
  next.splice(target, 0, item)
  return { changed: true, entries: next, feedback: describeMove(entries, next, id) }
}

function moveToRestIndex(
  entries: readonly RailLayoutEntry[],
  id: RailItemId,
  index: number
): RailReorderResult {
  const current = itemIndex(entries, id)
  if (current < 0 && !isUtilityItemId(id)) return unchanged(entries, id, 'same')
  const rest =
    current < 0
      ? entries.map(cloneEntry)
      : entries.filter((_, entryIndex) => entryIndex !== current)
  const clamped = Math.min(rest.length, Math.max(0, index))
  if (current >= 0 && clamped === current) return unchanged(entries, id, 'same')
  const next = rest.map(cloneEntry)
  next.splice(clamped, 0, { kind: 'item', id })
  if (sameRailEntries(entries, next)) return unchanged(entries, id, 'same')
  return { changed: true, entries: next, feedback: describeMove(entries, next, id) }
}

function describeMove(
  before: readonly RailLayoutEntry[],
  after: readonly RailLayoutEntry[],
  id: RailItemId
): RailReorderFeedback {
  const oldIndex = itemIndex(before, id)
  const newIndex = itemIndex(after, id)
  if (newIndex < 0) {
    return isUtilityItemId(id) ? { kind: 'pinned', id } : { kind: 'unchanged', reason: 'same', id }
  }
  const movedEarlier = oldIndex < 0 ? newIndex === 0 : newIndex < oldIndex
  if (movedEarlier) {
    const next = after[newIndex + 1]
    if (next) {
      return { kind: 'moved-before', id, neighborKind: next.kind, neighborId: next.id }
    }
  }
  const previous = after[newIndex - 1]
  if (previous) {
    return { kind: 'moved-after', id, neighborKind: previous.kind, neighborId: previous.id }
  }
  const next = after[newIndex + 1]
  if (next) {
    return { kind: 'moved-before', id, neighborKind: next.kind, neighborId: next.id }
  }
  return { kind: 'unchanged', reason: 'same', id }
}

function unchanged(
  entries: readonly RailLayoutEntry[],
  id: RailItemId,
  reason: 'first' | 'last' | 'same'
): RailReorderResult {
  return {
    changed: false,
    entries: entries.map(cloneEntry),
    feedback: { kind: 'unchanged', reason, id }
  }
}

function itemIndex(entries: readonly RailLayoutEntry[], id: RailItemId): number {
  return entries.findIndex((entry) => entry.kind === 'item' && entry.id === id)
}

function parseEntry(value: unknown): RailLayoutEntry | null {
  if (!isRecord(value)) return null
  for (const key of Object.keys(value)) {
    if (isForbiddenCredentialKey(key)) contractFail(FORBIDDEN)
    if (key !== 'kind' && key !== 'id') contractFail(INVALID)
  }
  if (value.kind === 'item' && typeof value.id === 'string' && isRailItemId(value.id)) {
    return { kind: 'item', id: value.id }
  }
  if (value.kind === 'divider' && typeof value.id === 'string' && isDividerId(value.id)) {
    return { kind: 'divider', id: value.id }
  }
  return null
}

function insertAfterNearestPredecessor(entries: RailLayoutEntry[], entry: RailLayoutEntry): void {
  const expectedIndex = defaultIndex(entry)
  let predecessorIndex = -1
  for (let index = expectedIndex - 1; index >= 0; index -= 1) {
    const candidate = DEFAULT_ENTRIES[index]
    const found = entries.findIndex(
      (item) => item.kind === candidate.kind && item.id === candidate.id
    )
    if (found >= 0) {
      predecessorIndex = found
      break
    }
  }
  const inserted = cloneEntry(entry)
  if (predecessorIndex < 0) entries.unshift(inserted)
  else entries.splice(predecessorIndex + 1, 0, inserted)
}

function defaultIndex(entry: RailLayoutEntry): number {
  const index = DEFAULT_ENTRIES.findIndex(
    (candidate) => candidate.kind === entry.kind && candidate.id === entry.id
  )
  if (index < 0) contractFail(INVALID)
  return index
}

function entryKey(entry: RailLayoutEntry): string {
  return `${entry.kind}:${entry.id}`
}

function cloneEntry(entry: RailLayoutEntry): RailLayoutEntry {
  return entry.kind === 'item' ? { kind: 'item', id: entry.id } : { kind: 'divider', id: entry.id }
}

function isRailItemId(value: string): value is RailItemId {
  return (RAIL_ITEM_IDS as readonly string[]).includes(value)
}

function isUtilityItemId(value: string): value is RailUtilityItemId {
  return (RAIL_UTILITY_ITEM_IDS as readonly string[]).includes(value)
}

const USER_DIVIDER_ID = /^sep-[1-9][0-9]{0,5}$/

function nextUserDividerId(entries: readonly RailLayoutEntry[]): RailUserDividerId {
  const used = new Set(entries.map((entry) => entry.id))
  let n = 1
  while (used.has(`sep-${n}`)) n += 1
  return `sep-${n}`
}

function isDividerId(value: string): value is RailDividerId {
  return (RAIL_DIVIDER_IDS as readonly string[]).includes(value) || isUserDividerId(value)
}
