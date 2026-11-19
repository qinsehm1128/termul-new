import { toast } from 'sonner'
import { runtimeT } from '@/i18n/runtime'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { useEditorStore } from '@/stores/editor-store'
import { useFloatingDocStore } from '@/stores/floating-doc-store'
import { useProjectStore } from '@/stores/project-store'
import { useWorkspaceStore } from '@/stores/workspace-store'
import type { Project } from '@/types/project'

export interface ExternalFileRouteResult {
  opened: string[]
  failed: Array<{ path: string; error: string }>
}

function normalizePath(path: string): string {
  const unified = path.replace(/\\/g, '/')
  const stripped = unified.length > 1 ? unified.replace(/\/+$/, '') : unified
  // macOS `/tmp` and `/var` resolve to `/private/tmp` and `/private/var` after canonicalize.
  if (stripped === '/private/tmp' || stripped.startsWith('/private/tmp/')) {
    return stripped.slice('/private'.length)
  }
  if (stripped === '/private/var' || stripped.startsWith('/private/var/')) {
    return stripped.slice('/private'.length)
  }
  return stripped
}

function isCaseInsensitivePath(path: string): boolean {
  if (/^[A-Za-z]:\//.test(path)) return true
  if (typeof navigator !== 'undefined' && /mac|win/i.test(navigator.platform)) return true
  return false
}

function pathBelongsToProject(filePath: string, projectPath: string): boolean {
  const file = normalizePath(filePath)
  const root = normalizePath(projectPath)
  if (!root) return false
  if (file === root || file.startsWith(`${root}/`)) return true
  if (isCaseInsensitivePath(file) || isCaseInsensitivePath(root)) {
    const lowerFile = file.toLowerCase()
    const lowerRoot = root.toLowerCase()
    return lowerFile === lowerRoot || lowerFile.startsWith(`${lowerRoot}/`)
  }
  return false
}

/**
 * Project that owns `filePath`: the longest project path prefix, ignoring
 * archived projects. Ties keep the first match, which is the sidebar order.
 */
export function findOwningProject(
  filePath: string,
  projects: Project[],
  defaultProjectId?: string | null
): Project | null {
  let owner: Project | null = null
  let ownerLength = -1
  for (const project of projects) {
    if (project.isArchived || !project.path) continue
    if (!pathBelongsToProject(filePath, project.path)) continue
    const length = normalizePath(project.path).length
    if (length > ownerLength) {
      owner = project
      ownerLength = length
    }
  }
  if (owner) return owner

  if (!defaultProjectId) return null
  const fallback = projects.find(
    (project) => project.id === defaultProjectId && !project.isArchived
  )
  return fallback ?? null
}

async function openWithConfiguredTarget(path: string): Promise<void> {
  const target = useAppSettingsStore.getState().settings.externalFileOpenTarget
  if (target === 'floating') {
    await useFloatingDocStore.getState().openDocument(path)
    return
  }
  await useEditorStore.getState().openFile(path)
  useWorkspaceStore.getState().addEditorTab(path)
}

/**
 * Show files handed over by the operating system (Finder "Open With", dock
 * drops, a browser query string). Files inside a known project switch to that
 * project first so the tab they open into belongs to the right workspace.
 */
export async function routeExternalFileOpen(paths: string[]): Promise<ExternalFileRouteResult> {
  const result: ExternalFileRouteResult = { opened: [], failed: [] }
  const unique = Array.from(new Set(paths.filter((path) => path.trim().length > 0)))
  if (unique.length === 0) return result

  const defaultProjectId = useAppSettingsStore.getState().settings.externalFileDefaultProjectId

  for (const path of unique) {
    const { projects, activeProjectId } = useProjectStore.getState()
    const owner = findOwningProject(path, projects, defaultProjectId)
    if (owner && owner.id !== activeProjectId) {
      useProjectStore.getState().selectProject(owner.id)
    }
    try {
      await openWithConfiguredTarget(path)
      result.opened.push(path)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      result.failed.push({ path, error: message })
      toast.error(
        runtimeT('settings', 'floatingDoc.openFailed', 'Could not open {{path}}', { path }),
        { description: message }
      )
    }
  }

  return result
}
