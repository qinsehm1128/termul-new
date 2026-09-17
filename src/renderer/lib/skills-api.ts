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

export type SkillInstallSource =
  | { type: 'local'; sourcePath: string }
  | { type: 'github'; repositoryOrUrl: string; reference?: string | null; subpath?: string | null }
  | { type: 'npm'; package: string; versionOrSpecifier?: string | null; subpath?: string | null }
  | { type: 'url'; url: string; expectedSha256?: string | null }

export type SkillScope = { type: 'global' } | { type: 'project'; projectId: string }
export type SkillInstallMode = 'installAndProject' | 'installOnly' | 'projectionOnly'

export interface SkillsPreviewRequest {
  source: SkillInstallSource
  scope: SkillScope
  mode?: SkillInstallMode
  providerIds?: string[]
}

export interface SkillsOperationStart {
  jobId: string
}

export interface SkillsInstallPlan {
  source: SkillInstallSource
  scope: SkillScope
  mode: SkillInstallMode
  providerIds: string[]
  previewId: string
  name: string
  actualSha256: string
  expectedSha256?: string | null
  requiresDigestConfirmation?: boolean
  collisionConfirmToken?: string | null
}

export interface SkillsInstallCommit {
  previewId: string
  confirmDigest?: boolean
  confirmToken?: string | null
}

export interface SkillsOperationStatus {
  jobId: string
  phase: string
  progress?: {
    jobId: string
    phase: string
    completedUnits?: number | null
    totalUnits?: number | null
    bytesReceived?: number | null
    bytesTotal?: number | null
    stableCode?: string | null
    message?: string | null
  } | null
  result?: unknown
  errorCode?: string | null
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
  source?: {
    sourceType?: string | null
    normalizedLocator?: string | null
    reference?: string | null
    resolvedVersion?: string | null
    resolvedCommit?: string | null
    actualSha256: string
  } | null
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
export const SKILLS_OPERATION_PROGRESS_EVENT = 'skills_operation_progress'

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

  async preview(request: SkillsPreviewRequest): Promise<SkillsOperationStart> {
    if (!isTauriContext()) return webServerSkills.preview(request)
    return invoke<SkillsOperationStart>('skills_preview_cmd', { request })
  },

  async installPreview(request: SkillsInstallCommit): Promise<SkillsOperationStart> {
    if (!isTauriContext()) return webServerSkills.installPreview(request)
    return invoke<SkillsOperationStart>('skills_install_preview_cmd', { request })
  },

  async operationStatus(jobId: string): Promise<SkillsOperationStatus> {
    if (!isTauriContext()) return webServerSkills.operationStatus(jobId)
    return invoke<SkillsOperationStatus>('skills_operation_status_cmd', { jobId })
  },

  async cancelOperation(jobId: string): Promise<void> {
    if (!isTauriContext()) return webServerSkills.cancelOperation(jobId)
    await invoke<void>('skills_cancel_operation_cmd', { jobId })
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

  onOperationProgress(listener: (status: SkillsOperationStatus) => void): () => void {
    if (!isTauriContext()) return () => undefined
    const unlisten = listen<SkillsOperationStatus>(SKILLS_OPERATION_PROGRESS_EVENT, (event) => {
      listener(event.payload)
    })
    return () => cleanupTauriListener(unlisten)
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
