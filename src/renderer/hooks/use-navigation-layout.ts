import { normalizeRailLayout } from '@shared/types/navigation.types'
import { useEffect } from 'react'
import { persistenceApi } from '@/lib/persistence-api'
import { RAIL_LAYOUT_STORAGE_KEY, useNavigationStore } from '@/stores/navigation-store'

let loadPromise: Promise<void> | null = null
let hasSubscribedToPersistence = false

function layoutDocument(value: unknown): string {
  return JSON.stringify(value)
}

async function initializeNavigationLayout(): Promise<void> {
  try {
    const result = await persistenceApi.read<unknown>(RAIL_LAYOUT_STORAGE_KEY)
    if (useNavigationStore.getState().layout.revision > 0) {
      useNavigationStore.setState({ isLoaded: true, loadFailed: false })
      void persistenceApi.writeDebounced(
        RAIL_LAYOUT_STORAGE_KEY,
        useNavigationStore.getState().layout
      )
      return
    }

    if (!result.success) {
      if (result.code === 'KEY_NOT_FOUND') {
        useNavigationStore.setState({ isLoaded: true, loadFailed: false })
        return
      }
      useNavigationStore.setState({ loadFailed: true })
      return
    }

    if (result.data == null) {
      useNavigationStore.setState({ isLoaded: true, loadFailed: false })
      return
    }

    let normalized: ReturnType<typeof normalizeRailLayout>
    try {
      normalized = normalizeRailLayout(result.data)
    } catch {
      useNavigationStore.setState({ loadFailed: true })
      return
    }

    if (useNavigationStore.getState().layout.revision > 0) {
      useNavigationStore.setState({ isLoaded: true, loadFailed: false })
      void persistenceApi.writeDebounced(
        RAIL_LAYOUT_STORAGE_KEY,
        useNavigationStore.getState().layout
      )
      return
    }

    const changed = layoutDocument(result.data) !== layoutDocument(normalized)
    useNavigationStore.getState().hydrateLayout(normalized)
    if (changed) {
      await persistenceApi.write(RAIL_LAYOUT_STORAGE_KEY, useNavigationStore.getState().layout)
    }
  } catch {
    useNavigationStore.setState({ loadFailed: true })
  }
}

/**
 * Loads and saves the global activity-rail order. Route targets, current-page
 * state, tooltips, and the command palette do not read this layout.
 */
export function useNavigationLayout(): void {
  useEffect(() => {
    if (!hasSubscribedToPersistence) {
      useNavigationStore.subscribe((state, previous) => {
        if (!state.isLoaded || state.loadFailed || !previous.isLoaded) return
        if (layoutDocument(state.layout) === layoutDocument(previous.layout)) return
        void persistenceApi.writeDebounced(RAIL_LAYOUT_STORAGE_KEY, state.layout)
      })
      hasSubscribedToPersistence = true
    }

    const state = useNavigationStore.getState()
    if (!loadPromise && !state.isLoaded && !state.loadFailed) {
      loadPromise = initializeNavigationLayout()
    }
  }, [])
}

export function resetNavigationLayoutLoaderForTests(): void {
  loadPromise = null
}
