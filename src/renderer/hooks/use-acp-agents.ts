import type { LastSelectedAgent } from '@shared/types/persistence.types'
import { PersistenceKeys } from '@shared/types/persistence.types'
import { useEffect, useRef } from 'react'
import type { StoredAgentConfig } from '@/lib/acp-agents-persistence'
import {
  pickDefaultSupportedAgent,
  resolveSupportedAcpAgents
} from '@/lib/agents/supported-acp-agents'
import { persistenceApi } from '@/lib/api'
import { getDefaultCwdForProject } from '@/lib/worktree-context'
import { useAcpStore } from '@/stores/acp-store'
import { useProjectStore } from '@/stores/project-store'

/**
 * How long a project switch waits before warming an agent for the new cwd.
 *
 * `prewarmAgent` spawns — or retargets — a real agent process, and
 * `retargetWarmPool` moves the warm-session pool with it. Clicking through a
 * few tabs to reach the one you want would otherwise start that work once per
 * project passed through on the way.
 */
const PREWARM_COALESCE_MS = 400

/**
 * The supported-agent catalog, memoised against the configs it was built from.
 *
 * `resolveSupportedAcpAgents` is a host round-trip over every registry agent —
 * the part that made switching projects expensive. It depends only on
 * `agentConfigs`, so caching it against that array's identity is safe: saving a
 * config replaces the array and invalidates the entry on its own.
 */
type AgentCatalogCache = {
  configs: StoredAgentConfig[]
  agents: ReturnType<typeof resolveSupportedAcpAgents>
}

/**
 * Pick the agent to warm: the last-selected ready supported ACP agent, falling
 * back to the default ready entry.
 *
 * Nothing here depends on the active project, but it does depend on the user's
 * current choice. The whole result used to be memoised for the app's lifetime,
 * which meant picking a different agent in the launcher — which writes
 * `lastSelectedAgent` (`AgentLauncher.tsx`) — was silently undone by the next
 * project switch: the stale id was republished through
 * `setSelectedAgentConfigId` and a process was warmed for the wrong agent. Only
 * the catalog is cached now; the choice is re-read every run.
 *
 * Returns the config id to warm, or `null` when nothing is ready.
 */
async function resolveAgentConfigIdToWarm(
  configsLoaded: Promise<void>,
  saveAgentConfig: (config: StoredAgentConfig) => Promise<void>,
  catalog: { current: AgentCatalogCache | null }
): Promise<string | null> {
  await configsLoaded
  const { agentConfigs } = useAcpStore.getState()
  if (catalog.current?.configs !== agentConfigs) {
    catalog.current = { configs: agentConfigs, agents: resolveSupportedAcpAgents(agentConfigs) }
  }
  const supportedAgents = await catalog.current.agents
  const persisted = await persistenceApi.read<unknown>(PersistenceKeys.lastSelectedAgent)
  const saved = persisted.success ? (persisted.data as Partial<LastSelectedAgent> | null) : null
  const selected =
    saved?.mode === 'acp' && typeof saved.agentId === 'string'
      ? supportedAgents.find(
          (entry) => entry.configId === saved.agentId && entry.status === 'ready'
        )
      : null
  const entry = selected ?? pickDefaultSupportedAgent(supportedAgents)
  if (!entry?.config) return null
  if (!agentConfigs.some((config) => config.id === entry.config?.id)) {
    await saveAgentConfig(entry.config)
  }
  return entry.config.id
}

/**
 * Load persisted ACP agent configs once at app mount, then resolve the
 * last-selected ready supported ACP agent (falling back to the default ready
 * entry) and publish it as the warm-pool target. The hook prewarms that agent's
 * process and seeds its warm-session pool for the active project cwd; re-runs on
 * project switch. Agent Chat derives supported configs automatically, so prewarm
 * must not fan out across every supported agent or depend on Preferences toggles.
 */
export function useAcpAgents(): void {
  const loadAgentConfigs = useAcpStore((s) => s.loadAgentConfigs)
  const saveAgentConfig = useAcpStore((s) => s.saveAgentConfig)
  const setSelectedAgentConfigId = useAcpStore((s) => s.setSelectedAgentConfigId)
  const retargetWarmPool = useAcpStore((s) => s.retargetWarmPool)
  const activeProjectId = useProjectStore((s) => s.activeProjectId)
  const configsLoadedRef = useRef<Promise<void> | null>(null)
  const catalogRef = useRef<AgentCatalogCache | null>(null)

  useEffect(() => {
    // Loading persisted configs is not project-dependent and must happen even
    // with no project selected — other surfaces read `agentConfigs` regardless.
    // Kept ahead of the cwd check for that reason, and memoised so a switch no
    // longer re-reads them.
    configsLoadedRef.current ??= loadAgentConfigs()
    const configsLoaded = configsLoadedRef.current

    const cwd = activeProjectId ? getDefaultCwdForProject(activeProjectId) : ''
    if (cwd.trim().length === 0) {
      setSelectedAgentConfigId(null)
      return
    }

    let cancelled = false
    let prewarmTimer: ReturnType<typeof setTimeout> | null = null

    // Resolved lazily rather than at mount so a run with no active project
    // still never touches the catalog, exactly as before the split. The
    // expensive half is memoised inside `catalogRef`; the user's current choice
    // is re-read here every run.
    void resolveAgentConfigIdToWarm(configsLoaded, saveAgentConfig, catalogRef)
      .catch(() => null)
      .then((configId) => {
        if (configId === null) {
          // Nothing was ready. Drop the catalog so the next switch looks again —
          // an agent installed mid-session should still be picked up.
          catalogRef.current = null
          if (!cancelled) setSelectedAgentConfigId(null)
          return
        }
        if (cancelled) return
        // Publishing the selection is cheap and drives UI, so it stays prompt;
        // only the process work below waits out the burst.
        setSelectedAgentConfigId(configId)
        prewarmTimer = setTimeout(() => {
          prewarmTimer = null
          void useAcpStore.getState().prewarmAgent(configId, cwd)
          retargetWarmPool(configId, cwd, activeProjectId)
        }, PREWARM_COALESCE_MS)
      })

    return () => {
      cancelled = true
      if (prewarmTimer) {
        clearTimeout(prewarmTimer)
        prewarmTimer = null
      }
    }
  }, [
    loadAgentConfigs,
    saveAgentConfig,
    setSelectedAgentConfigId,
    retargetWarmPool,
    activeProjectId
  ])
}
