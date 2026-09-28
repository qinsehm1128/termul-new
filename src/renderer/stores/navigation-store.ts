import {
  acceptRailRevision,
  applyRailReorder,
  createRailRevisionFence,
  defaultRailLayout,
  insertRailDivider,
  normalizeRailLayout,
  RAIL_LAYOUT_SCHEMA_VERSION,
  type RailDividerId,
  type RailItemId,
  type RailLayoutConfig,
  type RailReorderFeedback,
  removeRailDivider,
  sameRailEntries
} from '@shared/types/navigation.types'
import { create } from 'zustand'

/** Global activity-rail order. Not per project, and not read by the command palette. */
export const RAIL_LAYOUT_STORAGE_KEY = 'settings/activity-rail'

interface NavigationState {
  layout: RailLayoutConfig
  isLoaded: boolean
  loadFailed: boolean
  hydrateLayout: (value: unknown) => RailLayoutConfig
  moveItemBy: (id: RailItemId, delta: -1 | 1) => RailReorderFeedback
  moveItemToIndex: (id: RailItemId, index: number) => RailReorderFeedback
  pinUtility: (id: RailItemId) => RailReorderFeedback
  insertDivider: (beforeEntryIndex: number) => void
  removeDivider: (id: RailDividerId) => void
}

const railRevisionFence = createRailRevisionFence()

function commitEntries(
  entries: RailLayoutConfig['entries'],
  set: (partial: Partial<NavigationState>) => void,
  get: () => NavigationState
): void {
  const current = get().layout
  const revision = (railRevisionFence.lastAccepted ?? current.revision) + 1
  const normalized = normalizeRailLayout({
    schemaVersion: RAIL_LAYOUT_SCHEMA_VERSION,
    revision,
    entries
  })
  if (sameRailEntries(current.entries, normalized.entries)) return
  acceptRailRevision(railRevisionFence, normalized.revision)
  set({ layout: normalized, loadFailed: false })
}

export const useNavigationStore = create<NavigationState>((set, get) => ({
  layout: defaultRailLayout(),
  isLoaded: false,
  loadFailed: false,

  hydrateLayout: (value) => {
    const normalized = normalizeRailLayout(value)
    if (normalized.revision >= 1) railRevisionFence.lastAccepted = normalized.revision
    else railRevisionFence.lastAccepted = null
    set({ layout: normalized, isLoaded: true, loadFailed: false })
    return normalized
  },

  moveItemBy: (id, delta) => {
    const result = applyRailReorder(get().layout.entries, id, { kind: 'by', delta })
    if (result.changed) commitEntries(result.entries, set, get)
    return result.feedback
  },

  moveItemToIndex: (id, index) => {
    const result = applyRailReorder(get().layout.entries, id, { kind: 'to-index', index })
    if (result.changed) commitEntries(result.entries, set, get)
    return result.feedback
  },

  pinUtility: (id) => {
    const result = applyRailReorder(get().layout.entries, id, { kind: 'pin' })
    if (result.changed) commitEntries(result.entries, set, get)
    return result.feedback
  },

  insertDivider: (beforeEntryIndex) => {
    commitEntries(insertRailDivider(get().layout.entries, beforeEntryIndex), set, get)
  },

  removeDivider: (id) => {
    commitEntries(removeRailDivider(get().layout.entries, id), set, get)
  }
}))

export function resetNavigationStore(options?: { isLoaded?: boolean }): void {
  railRevisionFence.lastAccepted = null
  useNavigationStore.setState({
    layout: defaultRailLayout(),
    isLoaded: options?.isLoaded ?? true,
    loadFailed: false
  })
}
