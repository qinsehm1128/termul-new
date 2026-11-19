import { useEffect, useState } from 'react'
import {
  onExternalFileOpen,
  subscribeExternalFileOpen,
  supportsExternalFileOpen,
  takePendingExternalFiles
} from '@/lib/external-file-open-api'
import { routeExternalFileOpen } from '@/lib/external-file-router'
import { useAppSettingsLoaded } from '@/stores/app-settings-store'
import { useProjectStore } from '@/stores/project-store'

/** The hand-off surface this hook needs; injectable for tests. */
export interface ExternalFileHandoff {
  supportsExternalFileOpen: () => boolean
  takePendingExternalFiles: () => Promise<string[]>
  onExternalFileOpen: (callback: (paths: string[]) => void) => () => void
  subscribeExternalFileOpen?: (callback: (paths: string[]) => void) => Promise<() => void>
}

const defaultHandoff: ExternalFileHandoff = {
  supportsExternalFileOpen,
  takePendingExternalFiles,
  onExternalFileOpen,
  subscribeExternalFileOpen
}

const HYDRATION_FALLBACK_MS = 1500

/**
 * Paths handed over through the browser URL (`?openFile=...`), used by the
 * browser edition and as a manual test hook. Accepts repeated parameters and
 * comma-separated values.
 */
export function parseOpenFileQuery(search: string): string[] {
  const params = new URLSearchParams(search)
  const paths: string[] = []
  for (const [key, value] of params.entries()) {
    if (key !== 'openFile') continue
    for (const part of value.split(',')) {
      const trimmed = part.trim()
      if (trimmed) paths.push(trimmed)
    }
  }
  return paths
}

export interface UseExternalFileOpenOptions {
  /** Override the desktop hand-off (tests). */
  handoff?: ExternalFileHandoff | null
  /** Override the router (tests). Defaults to `routeExternalFileOpen`. */
  route?: (paths: string[]) => void
  /** Wait for persisted projects & app settings before draining cold-start paths. */
  waitForHydration?: boolean
  /** Fallback wait (ms) when hydration stalls (e.g. Web API error). */
  hydrationTimeoutMs?: number
}

/**
 * Opens files the desktop or the browser URL hands to the app. Mount once, near
 * the app root; every file goes through `routeExternalFileOpen` so project
 * attribution and the configured open target apply to all of them.
 *
 * Subscribes before draining the pending queue so no live open arrives between
 * `takePendingExternalFiles` and listener registration.
 */
export function useExternalFileOpen(options: UseExternalFileOpenOptions = {}): void {
  const {
    handoff = defaultHandoff,
    route,
    waitForHydration = options.handoff === undefined,
    hydrationTimeoutMs = HYDRATION_FALLBACK_MS
  } = options
  const settingsLoaded = useAppSettingsLoaded()
  const projectsLoaded = useProjectStore((state) => state.isLoaded)
  const [hydrationTimedOut, setHydrationTimedOut] = useState(false)
  const ready = !waitForHydration || (settingsLoaded && projectsLoaded) || hydrationTimedOut

  useEffect(() => {
    if (!waitForHydration || ready || hydrationTimeoutMs <= 0) return
    const timer = window.setTimeout(() => setHydrationTimedOut(true), hydrationTimeoutMs)
    return () => window.clearTimeout(timer)
  }, [hydrationTimeoutMs, ready, waitForHydration])

  useEffect(() => {
    if (!handoff || !ready) return
    let cancelled = false
    let unsubscribe: (() => void) | undefined
    const seenInDrainWindow = new Set<string>()
    let drainFinished = false
    const baseOpen = route ?? ((paths: string[]) => void routeExternalFileOpen(paths))
    const open = (paths: string[]): void => {
      if (drainFinished) {
        baseOpen(paths)
        return
      }
      const fresh = paths.filter((path) => {
        if (seenInDrainWindow.has(path)) return false
        seenInDrainWindow.add(path)
        return true
      })
      if (fresh.length > 0) baseOpen(fresh)
    }

    const startDesktopHandoff = async (): Promise<void> => {
      if (!handoff.supportsExternalFileOpen()) return
      if (typeof handoff.subscribeExternalFileOpen === 'function') {
        const unsub = await handoff.subscribeExternalFileOpen((paths) => {
          if (!cancelled && paths.length > 0) open(paths)
        })
        if (cancelled) {
          unsub()
          return
        }
        unsubscribe = unsub
      } else {
        unsubscribe = handoff.onExternalFileOpen((paths) => {
          if (!cancelled && paths.length > 0) open(paths)
        })
      }

      try {
        const paths = await handoff.takePendingExternalFiles()
        if (!cancelled && paths.length > 0) open(paths)
      } catch {
        // Missing hand-off must not block app startup.
      } finally {
        drainFinished = true
        seenInDrainWindow.clear()
      }
    }

    void startDesktopHandoff()

    if (typeof window !== 'undefined') {
      const fromQuery = parseOpenFileQuery(window.location.search)
      if (fromQuery.length > 0) baseOpen(fromQuery)
    }

    return () => {
      cancelled = true
      unsubscribe?.()
    }
  }, [handoff, ready, route])
}
