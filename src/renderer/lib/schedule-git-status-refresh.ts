import { useGitStatusStore } from '@/stores/git-status-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'

const DEBOUNCE_MS = 1000

function normalizePath(p: string): string {
  return p.replace(/\\/g, '/')
}

function shouldCompareCaseInsensitively(path: string, root: string): boolean {
  return (
    /^[a-zA-Z]:\//.test(path) ||
    /^[a-zA-Z]:\//.test(root) ||
    path.startsWith('//') ||
    root.startsWith('//')
  )
}

/** True when `filePath` is inside or equal to the git repo root `repoCwd`. */
export function isPathWithinRepo(filePath: string, repoCwd: string): boolean {
  const path = normalizePath(filePath)
  const root = normalizePath(repoCwd).replace(/\/$/, '')
  const caseInsensitive = shouldCompareCaseInsensitively(path, root)
  const comparablePath = caseInsensitive ? path.toLowerCase() : path
  const comparableRoot = caseInsensitive ? root.toLowerCase() : root
  return comparablePath === comparableRoot || comparablePath.startsWith(`${comparableRoot}/`)
}

/** Distinct `cwd` values from open Git Changes tabs. */
export function collectOpenGitTabCwds(): string[] {
  const root = useWorkspaceStore.getState().root
  const cwds = new Set<string>()
  for (const leaf of getAllLeafPanes(root)) {
    for (const tab of leaf.tabs) {
      if (tab.type === 'git') {
        cwds.add(tab.cwd)
      }
    }
  }
  return [...cwds]
}

const pendingCwds = new Set<string>()
let flushTimer: ReturnType<typeof setTimeout> | null = null

/** Open Git tab cwds for the current synchronous run, computed at most once. */
let cwdsThisTick: string[] | null = null

/**
 * The open Git tab cwds, memoised for one synchronous run.
 *
 * `collectOpenGitTabCwds` walks the whole pane tree and every tab on it. The fs
 * watcher delivers up to 500 changes per batch and dispatches them in a single
 * synchronous loop, so calling it per event meant walking that tree hundreds of
 * times for a set that cannot change between two events of the same batch —
 * and recursive root watching made those batches far more common than the old
 * per-expanded-directory model ever produced.
 *
 * A microtask clears it, so anything that opens or closes a Git tab is picked up
 * by the next batch. Deliberately a resolved promise rather than
 * `queueMicrotask`: fake timers in tests may replace the latter, and a memo that
 * never expires would be a far worse bug than the one this avoids.
 */
function openGitTabCwdsThisTick(): string[] {
  if (cwdsThisTick === null) {
    cwdsThisTick = collectOpenGitTabCwds()
    void Promise.resolve().then(() => {
      cwdsThisTick = null
    })
  }
  return cwdsThisTick
}

/**
 * Debounced refresh of git status for every open Git Changes tab whose repo
 * contains `filePath`. No-op when no matching git tab is open.
 */
export function scheduleGitStatusRefreshForPath(filePath: string): void {
  let matched = false
  for (const cwd of openGitTabCwdsThisTick()) {
    if (isPathWithinRepo(filePath, cwd)) {
      pendingCwds.add(cwd)
      matched = true
    }
  }
  if (!matched) return

  if (flushTimer) clearTimeout(flushTimer)
  flushTimer = setTimeout(() => {
    const refresh = useGitStatusStore.getState().refreshStatus
    const openCwds = new Set(collectOpenGitTabCwds())
    for (const cwd of pendingCwds) {
      if (openCwds.has(cwd)) {
        void refresh(cwd)
      }
    }
    pendingCwds.clear()
    flushTimer = null
  }, DEBOUNCE_MS)
}

/** Test-only reset of module-level debounce state. */
export function resetGitStatusRefreshSchedulerForTests(): void {
  pendingCwds.clear()
  if (flushTimer) clearTimeout(flushTimer)
  flushTimer = null
  cwdsThisTick = null
}
