/**
 * Read-only legacy workspace-manifest compatibility boundary.
 *
 * Project-keyed manifests are preserved migration evidence only. Normal renderer state never
 * writes or deletes them after the per-Conversation SessionWorkspace cutover. The inspection
 * helper deliberately does not restore terminal references because a shared project manifest
 * cannot prove Conversation or PtyManager ownership.
 */

import type {
  PaneNode as PortablePaneNode,
  WorkspaceManifest
} from '@shared/types/workspace-manifest.types'
import { workspaceManifestApi } from '@/lib/workspace-manifest-api'
import { useEditorStore } from '@/stores/editor-store'
import type { WorkspaceTab } from '@/stores/workspace-store'
import { editorTabId } from '@/stores/workspace-store'
import type { LeafNode, PaneNode, SplitNode } from '@/types/workspace.types'

export function buildPortableManifest(projectId: string): WorkspaceManifest {
  return {
    projectId,
    revision: 0,
    updatedAt: 0,
    terminals: [],
    editors: [],
    topology: undefined,
    activePaneId: null,
    focusedSessionId: null
  }
}

export function rebuildTopologyFromManifest(manifest: WorkspaceManifest): {
  root: PaneNode
  activePaneId: string | null
  focusedSessionId: string | null
} {
  const editors = new Map(manifest.editors.map((editor) => [editor.editorId, editor] as const))
  const rebuild = (node: PortablePaneNode): PaneNode => {
    if (node.type === 'split') {
      const split: SplitNode = {
        type: 'split',
        id: node.id,
        direction: node.direction,
        children: node.children.map(rebuild),
        sizes: node.sizes
      }
      return split
    }
    const tabs: WorkspaceTab[] = []
    for (const editorId of node.editorIds) {
      const editor = editors.get(editorId)
      if (editor) {
        tabs.push({ type: 'editor', id: editorTabId(editor.filePath), filePath: editor.filePath })
      }
    }
    const activeTabId =
      node.activeTabId && tabs.some((tab) => tab.id === node.activeTabId)
        ? node.activeTabId
        : (tabs[0]?.id ?? null)
    const leaf: LeafNode = { type: 'leaf', id: node.id, tabs, activeTabId }
    return leaf
  }
  const root = manifest.topology
    ? rebuild(manifest.topology)
    : ({ type: 'leaf', id: crypto.randomUUID(), tabs: [], activeTabId: null } satisfies LeafNode)
  return {
    root,
    activePaneId: manifest.activePaneId ?? null,
    focusedSessionId: null
  }
}

export async function inspectLegacyWorkspaceManifest(
  projectId: string
): Promise<WorkspaceManifest | null> {
  if (!projectId) return null
  const result = await workspaceManifestApi.getManifest(projectId)
  return result.success ? result.data : null
}

/** @deprecated Legacy manifests are inspection-only and no longer restore live workspace state. */
export async function loadWorkspaceManifest(projectId: string): Promise<boolean> {
  const manifest = await inspectLegacyWorkspaceManifest(projectId)
  if (!manifest) return false
  // Preserve editor evidence for migration inspection without changing the pane tree or terminals.
  const editorStore = useEditorStore.getState()
  await Promise.all(
    manifest.editors.map((editor) =>
      editorStore.openFiles.has(editor.filePath)
        ? Promise.resolve()
        : editorStore.openFile(editor.filePath).catch(() => undefined)
    )
  )
  return false
}

export type ManifestWriteResult = 'updated' | 'conflict' | 'failed' | 'skipped'

/** @deprecated Project workspace manifests are permanently read-only after cutover. */
export async function performManifestWrite(_projectId: string): Promise<ManifestWriteResult> {
  return 'skipped'
}

/** @deprecated Legacy conflicts are not mutated; reload performs read-only inspection only. */
export async function resolveManifestConflict(
  projectId: string,
  action: 'reload' | 'overwrite' | 'dismiss'
): Promise<void> {
  if (action === 'reload') await inspectLegacyWorkspaceManifest(projectId)
}

/** @deprecated No live subscriptions or project-manifest writes remain. */
export function useWorkspaceManifestSync(_projectId: string): void {
  // Intentionally empty: SessionWorkspace owns normal Conversation-scoped synchronization.
}
