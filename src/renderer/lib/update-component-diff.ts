import {
  UPDATE_COMPONENTS,
  type UpdateComponent,
  type UpdateComponentPolicy
} from '@shared/types/updater.types'
import { invoke } from '@tauri-apps/api/core'

/** Live identity of one Core endpoint, as reported by `component_runtime_identities`. */
export interface CoreRuntimeIdentity {
  buildId: string | null
  activeResources: number
}

/** Build identities this install is running; a Core that is not listening is `null`. */
export interface ComponentRuntimeIdentities {
  guiNative: string
  acpCore: CoreRuntimeIdentity | null
  terminalCore: CoreRuntimeIdentity | null
}

/**
 * - `same`: the running build already matches the release.
 * - `replace`: the running build differs (or reports no ID) and is replaced.
 * - `bundled`: the renderer has no running build ID; it is replaced together
 *   with the app bundle.
 * - `notRunning`: the Core is not listening; its next start uses the new build.
 */
export type ComponentDiffStatus = 'same' | 'replace' | 'bundled' | 'notRunning'

export interface ComponentDiffRow {
  component: UpdateComponent
  currentBuildId: string | null
  nextBuildId: string
  status: ComponentDiffStatus
  /** Running terminals (Terminal Core) or agent sessions (ACP Core); null otherwise. */
  activeResources: number | null
}

/** Desktop-only: the web build has no local Cores to compare. */
export function fetchComponentRuntimeIdentities(): Promise<ComponentRuntimeIdentities> {
  return invoke<ComponentRuntimeIdentities>('component_runtime_identities')
}

function currentIdentity(
  component: UpdateComponent,
  runtime: ComponentRuntimeIdentities
): { running: boolean; buildId: string | null; activeResources: number | null } {
  switch (component) {
    case 'renderer':
      // No baked build ID: the renderer ships inside the app bundle.
      return { running: true, buildId: null, activeResources: null }
    case 'guiNative':
      return { running: true, buildId: runtime.guiNative, activeResources: null }
    case 'acpCore':
    case 'terminalCore': {
      const core = runtime[component]
      return core
        ? { running: true, buildId: core.buildId, activeResources: core.activeResources }
        : { running: false, buildId: null, activeResources: null }
    }
  }
}

export function buildComponentDiff(
  policy: UpdateComponentPolicy,
  runtime: ComponentRuntimeIdentities
): ComponentDiffRow[] {
  return UPDATE_COMPONENTS.map((component) => {
    const nextBuildId = policy.components[component].buildId
    const current = currentIdentity(component, runtime)
    const status: ComponentDiffStatus =
      component === 'renderer'
        ? 'bundled'
        : !current.running
          ? 'notRunning'
          : current.buildId === nextBuildId
            ? 'same'
            : 'replace'
    return {
      component,
      currentBuildId: current.buildId,
      nextBuildId,
      status,
      activeResources: current.activeResources
    }
  })
}

/** Terminals and agent sessions a forced install ends: only Cores being replaced count. */
export function forcedUpdateCasualties(rows: readonly ComponentDiffRow[]): {
  terminals: number
  agentSessions: number
} {
  const count = (component: UpdateComponent): number => {
    const row = rows.find((candidate) => candidate.component === component)
    return row?.status === 'replace' ? (row.activeResources ?? 0) : 0
  }
  return { terminals: count('terminalCore'), agentSessions: count('acpCore') }
}

const HASHED_BUILD_ID = /sha256:([0-9a-f]{12,})$/

/** Release IDs end in a 64-hex digest; show its last 12 characters. Other IDs show as-is. */
export function shortBuildId(buildId: string): string {
  const match = HASHED_BUILD_ID.exec(buildId)
  return match ? match[1].slice(-12) : buildId
}
