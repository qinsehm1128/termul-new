/**
 * Agent Skills IPC facade — lists and reads Zed-compatible SKILL.md packages.
 *
 * Skill discovery reads the user's local filesystem (`~/.agents/skills/` +
 * `{project}/.agents/skills/`). On desktop the Tauri commands
 * `list_agent_skills_cmd` / `read_agent_skill_cmd` back this facade; on the
 * web/remote client the same calls hit the shipped parity routes `GET /skills`
 * + `GET /skills/:name` (see `webServerSkills`), so the slash menu is usable
 * on both surfaces. `projectRoot` is optional — when omitted, only global
 * skills are listed; on web the route degrades to an empty list on scan
 * failure (never throws, so the slash menu stays usable).
 */
import { invoke } from '@tauri-apps/api/core'
import { listen } from './tauri-event'
import { cleanupTauriListener, isTauriContext } from './tauri-runtime'
import { webServerSkills } from './web-server-api'

export interface SkillSource {
  provider: string
  scope: string
  projectId?: string | null
  skillMdPath: string
  digest: string
  metadataDigest: string
}

export interface SkillRecord {
  name: string
  description: string
  scope: string
  projectId?: string | null
  digest: string
  metadataDigest: string
  status: string
  conflict?: boolean
  drift?: boolean
  managed?: boolean
  sources: SkillSource[]
}

export interface SkillsCatalog {
  revision: number
  skills: SkillRecord[]
  diagnostics: string[]
}

export interface SkillsStatus {
  catalog: SkillsCatalog
  fallbackPolicy: 'ask' | 'copy' | 'deny'
  stale?: boolean
  watchedRoots?: string[]
  lastScanAt?: number | null
}

export interface SkillsCatalogRequest {
  projectId?: string | null
  projectRoot?: string | null
}

export interface SkillsInstallRequest {
  name: string
  sourcePath: string
  scope?: string
  projectId?: string | null
  projectRoot?: string | null
  confirmToken?: string | null
  fallback?: 'ask' | 'copy' | 'deny' | null
}

export interface SkillsProjectionRequest {
  name: string
  projectId?: string | null
  projectRoot?: string | null
  confirmFallback?: boolean | null
  fallback?: 'ask' | 'copy' | 'deny' | null
}

export interface SkillsRepairRequest {
  name: string
  projectId?: string | null
  projectRoot?: string | null
}

export interface SkillManifest {
  name: string
  digest: string
  canonicalPath: string
  projections: Array<{
    provider: string
    targetPath: string
    mode: string
    sourceDigest: string
    targetDigest: string
    fallbackReason?: string | null
  }>
}

export interface SkillsHubEvent {
  kind: string
  revision: number
  projectId?: string | null
  paths?: string[]
}

export const SKILLS_CATALOG_CHANGED_EVENT = 'skills_catalog_changed'
export const SKILLS_CONFLICT_DETECTED_EVENT = 'skills_conflict_detected'
export const SKILLS_LINK_DRIFT_DETECTED_EVENT = 'skills_link_drift_detected'
export const SKILLS_SYNC_STALE_EVENT = 'skills_sync_stale'

export interface AgentSkillSummary {
  name: string
  description: string
  /** `'global'` or `'project'`. */
  scope: string
  /** Absolute path to the skill's `SKILL.md` so the wire prompt can cite it
   * (the agent reads the body from disk; no body is shipped over the wire). */
  path: string
}

export interface AgentSkillContent {
  name: string
  description: string
  scope: string
  body: string
  /** Absolute path to the skill's `SKILL.md`. */
  path: string
}

export const skillsApi = {
  listSkills(projectRoot?: string): Promise<AgentSkillSummary[]> {
    if (!isTauriContext()) return webServerSkills.list(projectRoot)
    return invoke<AgentSkillSummary[]>('list_agent_skills_cmd', {
      projectRoot: projectRoot || null
    })
  },

  async status(request: SkillsCatalogRequest = {}): Promise<SkillsStatus> {
    if (!isTauriContext()) return webServerSkills.status(request)
    return invoke<SkillsStatus>('skills_status_cmd', {
      projectId: request.projectId ?? null,
      projectRoot: request.projectRoot ?? null
    })
  },

  async refresh(request: SkillsCatalogRequest = {}): Promise<SkillsStatus> {
    if (!isTauriContext()) return webServerSkills.sync(request)
    return invoke<SkillsStatus>('skills_refresh_cmd', {
      projectId: request.projectId ?? null,
      projectRoot: request.projectRoot ?? null
    })
  },

  async sync(request: SkillsCatalogRequest = {}): Promise<SkillsStatus> {
    if (!isTauriContext()) return webServerSkills.sync(request)
    return invoke<SkillsStatus>('skills_sync_cmd', {
      projectId: request.projectId ?? null,
      projectRoot: request.projectRoot ?? null
    })
  },

  async install(request: SkillsInstallRequest): Promise<SkillManifest> {
    if (!isTauriContext()) return webServerSkills.install(request)
    return invoke<SkillManifest>('skills_install_cmd', { request })
  },

  async project(request: SkillsProjectionRequest): Promise<SkillManifest> {
    if (!isTauriContext()) return webServerSkills.project(request)
    return invoke<SkillManifest>('skills_project_cmd', { request })
  },

  async repair(request: SkillsRepairRequest): Promise<SkillManifest> {
    if (!isTauriContext()) return webServerSkills.repair(request)
    return invoke<SkillManifest>('skills_repair_cmd', { request })
  },

  readSkill(name: string, projectRoot?: string): Promise<AgentSkillContent> {
    if (!isTauriContext()) return webServerSkills.read(name, projectRoot)
    return invoke<AgentSkillContent>('read_agent_skill_cmd', {
      name,
      projectRoot: projectRoot || null
    })
  },

  onCatalogChanged(listener: (event: SkillsHubEvent) => void): () => void {
    if (!isTauriContext()) return webServerSkills.onCatalogChanged(listener)
    const pending = [
      SKILLS_CATALOG_CHANGED_EVENT,
      SKILLS_CONFLICT_DETECTED_EVENT,
      SKILLS_LINK_DRIFT_DETECTED_EVENT,
      SKILLS_SYNC_STALE_EVENT
    ].map((kind) =>
      listen<SkillsHubEvent>(kind, (event) => {
        listener(event.payload)
      })
    )
    return () => {
      for (const unlisten of pending) {
        cleanupTauriListener(unlisten)
      }
    }
  }
}
