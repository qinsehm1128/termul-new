import type { ConversationId } from '@shared/types/conversation.types'
import type {
  RecoveryActionName,
  RecoveryItemV1,
  ResolveRecoveryItemRequest
} from '@shared/types/conversation-recovery.types'
import type {
  EditorResourceDescriptor,
  SessionWorkspacePaneNode,
  SessionWorkspaceResourceDescriptor,
  SessionWorkspaceV1,
  TerminalResourceDescriptor
} from '@shared/types/session-workspace.types'
import { useEffect } from 'react'
import { isTerminalRestoreInProgress } from '@/hooks/useTerminalAutoSave'
import { logFrontendError } from '@/lib/log-api'
import { sessionWorkspaceApi } from '@/lib/session-workspace-api'
import { randomUUID } from '@/lib/uuid'
import { getCurrentConversation, useConversationStore } from '@/stores/conversation-store'
import { useEditorStore } from '@/stores/editor-store'
import { useProjectStore } from '@/stores/project-store'
import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'
import { useTerminalStore } from '@/stores/terminal-store'
import type { WorkspaceTab } from '@/stores/workspace-store'
import {
  editorTabId,
  retireTerminalRecord,
  terminalTabId,
  useWorkspaceStore
} from '@/stores/workspace-store'
import type { LeafNode, PaneNode, SplitNode } from '@/types/workspace.types'

const WRITE_DEBOUNCE_MS = 500
const alwaysCurrent = (): boolean => true
let updateIdentity: string | null = null

export type SessionWorkspaceActivationGuard = () => boolean

function rendererIdentity(): string {
  updateIdentity ??= `renderer-${randomUUID()}`
  return updateIdentity
}

export function getActiveConversationId(): ConversationId | null {
  return useConversationStore.getState().activeConversationId
}

function serializeTopology(
  node: PaneNode,
  conversationId: ConversationId
): SessionWorkspacePaneNode {
  if (node.type === 'split') {
    return {
      type: 'split',
      id: node.id,
      direction: node.direction,
      children: node.children.map((child) => serializeTopology(child, conversationId)),
      sizes: node.sizes
    }
  }
  const terminalIds: string[] = []
  const editorIds: string[] = []
  const terminals = useTerminalStore.getState().terminals
  for (const tab of node.tabs) {
    if (tab.type === 'terminal') {
      const terminal = terminals.find((candidate) => candidate.id === tab.terminalId)
      if (terminal?.conversationId === conversationId) terminalIds.push(tab.terminalId)
    }
    if (tab.type === 'editor') editorIds.push(editorTabId(tab.filePath))
  }
  return {
    type: 'leaf',
    id: node.id,
    terminalIds,
    editorIds,
    activeTabId: node.activeTabId
  }
}

function collectReferencedIds(
  node: SessionWorkspacePaneNode,
  terminals: Set<string>,
  editors: Set<string>
): void {
  if (node.type === 'split') {
    for (const child of node.children) collectReferencedIds(child, terminals, editors)
    return
  }
  for (const id of node.terminalIds) terminals.add(id)
  for (const id of node.editorIds) editors.add(id)
}

export function buildSessionWorkspace(conversationId: ConversationId): SessionWorkspaceV1 {
  const workspaceState = useWorkspaceStore.getState()
  const topology = serializeTopology(workspaceState.root, conversationId)
  const referencedTerminals = new Set<string>()
  const referencedEditors = new Set<string>()
  collectReferencedIds(topology, referencedTerminals, referencedEditors)
  const resources: SessionWorkspaceResourceDescriptor[] = []

  for (const terminal of useTerminalStore.getState().terminals) {
    if (terminal.conversationId !== conversationId || !terminal.ptyId) continue
    // A record the user has closed AND whose resume was refused is a dead end:
    // it renders nothing and cannot be revived. Keeping it in the manifest makes
    // `reconcileTerminalResources` re-materialize the tab on the next sync, so
    // closing it never sticks — that is where the phantom "Restored terminal"
    // comes from. A hidden record that is still healthy stays: it is a live PTY
    // the user can reopen.
    if (terminal.isHidden && terminal.healthStatus === 'disconnected') continue
    const descriptor: TerminalResourceDescriptor = {
      kind: 'terminal',
      terminalId: terminal.ptyId,
      terminalRecordId: terminal.id,
      conversationId
    }
    resources.push(descriptor)
  }

  for (const [filePath] of useEditorStore.getState().openFiles) {
    const editorId = editorTabId(filePath)
    if (!referencedEditors.has(editorId)) continue
    const descriptor: EditorResourceDescriptor = { kind: 'editor', editorId, filePath }
    resources.push(descriptor)
  }

  return {
    schemaVersion: 1,
    conversationId,
    revision: 0,
    updatedAtUtc: '',
    updateIdentity: rendererIdentity(),
    topology,
    activePaneId: workspaceState.activePaneId,
    resources,
    projectionState: { status: 'native' }
  }
}

function rebuildTopology(
  node: SessionWorkspacePaneNode,
  terminals: ReadonlyMap<string, TerminalResourceDescriptor>,
  editors: ReadonlyMap<string, EditorResourceDescriptor>,
  conversationId: ConversationId
): PaneNode {
  if (node.type === 'split') {
    const split: SplitNode = {
      type: 'split',
      id: node.id,
      direction: node.direction,
      children: node.children.map((child) =>
        rebuildTopology(child, terminals, editors, conversationId)
      ),
      sizes: node.sizes
    }
    return split
  }
  const tabs: WorkspaceTab[] = []
  const liveTerminals = useTerminalStore.getState().terminals
  for (const terminalId of node.terminalIds) {
    const descriptor = terminals.get(terminalId)
    const live = liveTerminals.find((terminal) => terminal.id === terminalId)
    const liveConversationId = live?.conversationId
    if (
      !descriptor ||
      descriptor.conversationId !== conversationId ||
      liveConversationId !== conversationId
    ) {
      continue
    }
    tabs.push({ type: 'terminal', id: terminalTabId(terminalId), terminalId })
  }
  for (const editorId of node.editorIds) {
    const descriptor = editors.get(editorId)
    if (!descriptor) continue
    tabs.push({
      type: 'editor',
      id: editorTabId(descriptor.filePath),
      filePath: descriptor.filePath
    })
  }
  const activeTabId =
    node.activeTabId && tabs.some((tab) => tab.id === node.activeTabId)
      ? node.activeTabId
      : (tabs[0]?.id ?? null)
  const leaf: LeafNode = { type: 'leaf', id: node.id, tabs, activeTabId }
  return leaf
}

function firstLeafId(node: PaneNode): string | null {
  if (node.type === 'leaf') return node.id
  for (const child of node.children) {
    const id = firstLeafId(child)
    if (id) return id
  }
  return null
}

function findLeafId(node: PaneNode, id: string): boolean {
  if (node.type === 'leaf') return node.id === id
  return node.children.some((child) => findLeafId(child, id))
}

function projectIdForConversation(conversationId: ConversationId): string {
  const conversation = getCurrentConversation(useConversationStore.getState(), conversationId)
  if (conversation?.projectAttachment?.projectId) return conversation.projectAttachment.projectId
  if (conversation && conversation.executionTarget.kind !== 'workspace') {
    return conversation.executionTarget.projectId
  }
  return useProjectStore.getState().activeProjectId
}

async function reconcileTerminalResources(
  conversationId: ConversationId,
  workspace: SessionWorkspaceV1,
  isCurrent: SessionWorkspaceActivationGuard
): Promise<boolean> {
  if (!isCurrent()) return false
  const descriptors = new Map<string, TerminalResourceDescriptor>()
  for (const resource of workspace.resources) {
    if (
      resource.kind === 'terminal' &&
      resource.conversationId === conversationId &&
      !descriptors.has(resource.terminalId)
    ) {
      descriptors.set(resource.terminalId, resource)
    }
  }

  const terminalStore = useTerminalStore.getState()
  const projectId = projectIdForConversation(conversationId)
  for (const descriptor of descriptors.values()) {
    if (!isCurrent()) return false
    // Materialize the passive record before requesting replay so the global
    // detached-output listener can capture bytes delivered during resume.
    terminalStore.hydrateTerminalResource(descriptor, undefined, projectId)
  }

  await Promise.all(
    Array.from(descriptors.values(), async (descriptor) => {
      if (!isCurrent()) return
      const recordId = descriptor.terminalRecordId ?? descriptor.terminalId
      const result = await useTerminalStore.getState().resumeTerminalResource(recordId)
      if (!result.success) {
        void logFrontendError({
          level: 'warn',
          source: 'session-workspace-sync.terminal-resume',
          message: `code=${result.code} conversationId=${conversationId} terminalId=${descriptor.terminalId}`
        })
        // `TERMINAL_GONE` can never be retried — the manifest reference
        // outlived its PTY, which is what every app exit leaves behind. Retire
        // record and tab together so the next manifest write stops carrying it
        // and the dead tab does not come back on the following launch.
        //
        // Deliberately AHEAD of the staleness guard: a gone PTY is a global
        // fact, not an activation-scoped one. Dropping it because the user
        // switched Conversations during the resume round-trip leaves the record
        // in the global store, where `syncTerminalTabs` materializes it as a
        // dead tab in whatever project is active next and `persistState` can
        // write that zombie into the saved layout. The guard below still covers
        // everything that IS activation-scoped.
        if (result.code === 'TERMINAL_GONE') retireTerminalRecord(recordId)
      }
      if (!isCurrent()) return
    })
  )
  return isCurrent()
}

function loadConversationWorkspace(
  conversationId: ConversationId,
  workspace: SessionWorkspaceV1,
  isCurrent: SessionWorkspaceActivationGuard
): boolean {
  if (!isCurrent()) return false
  const terminals = new Map<string, TerminalResourceDescriptor>()
  const editors = new Map<string, EditorResourceDescriptor>()
  for (const resource of workspace.resources) {
    if (resource.kind === 'terminal') {
      terminals.set(resource.terminalRecordId ?? resource.terminalId, resource)
    } else editors.set(resource.editorId, resource)
  }
  let root: PaneNode = workspace.topology
    ? rebuildTopology(workspace.topology, terminals, editors, conversationId)
    : { type: 'leaf', id: randomUUID(), tabs: [], activeTabId: null }
  let firstPaneId = firstLeafId(root)
  if (!firstPaneId) {
    firstPaneId = randomUUID()
    root = { type: 'leaf', id: firstPaneId, tabs: [], activeTabId: null }
  }
  const activePaneId =
    workspace.activePaneId && findLeafId(root, workspace.activePaneId)
      ? workspace.activePaneId
      : firstPaneId
  if (!isCurrent()) return false
  useWorkspaceStore.setState({ root, activePaneId })
  const editorStore = useEditorStore.getState()
  for (const editor of editors.values()) {
    if (!isCurrent()) return false
    if (!editorStore.openFiles.has(editor.filePath)) {
      void editorStore.openFile(editor.filePath).catch(() => undefined)
    }
  }
  return true
}

export async function loadSessionWorkspace(
  conversationId: ConversationId,
  isCurrent: SessionWorkspaceActivationGuard = alwaysCurrent
): Promise<boolean> {
  if (!isCurrent()) return false
  const store = useSessionWorkspaceSyncStore.getState()
  store.setRestoreInProgress(conversationId, true)
  try {
    const result = await sessionWorkspaceApi.getWorkspace(conversationId)
    if (!isCurrent()) return false
    if (!result.success) {
      void logFrontendError({
        source: 'session-workspace-sync',
        message: `workspace load failed code=${result.code} conversationId=${conversationId}`
      })
      return false
    }
    const outcome = result.data
    const outcomeConversationId =
      outcome.status === 'loaded' ? outcome.workspace.conversationId : outcome.conversationId
    if (outcomeConversationId !== conversationId) {
      void logFrontendError({
        level: 'error',
        source: 'session-workspace-sync',
        message: `conversationId=${conversationId} code=CONVERSATION_WORKSPACE_IDENTITY_MISMATCH`
      })
      return false
    }
    store.setLoadOutcome(conversationId, outcome)
    if (outcome.status === 'loaded') {
      const reconciled = await reconcileTerminalResources(
        conversationId,
        outcome.workspace,
        isCurrent
      )
      if (!isCurrent() || !reconciled) return false
      if (!loadConversationWorkspace(conversationId, outcome.workspace, isCurrent)) return false
      store.setBasedRevision(conversationId, outcome.workspace.revision)
      store.setRecoveryItems(conversationId, [])
      return true
    }
    if (!isCurrent()) return false
    store.setBasedRevision(conversationId, null)
    store.setRecoveryItems(
      conversationId,
      outcome.status === 'recoveryRequired' ? outcome.recoveryItems : []
    )
    return false
  } finally {
    if (isCurrent()) store.setRestoreInProgress(conversationId, false)
  }
}

export type SessionWorkspaceWriteResult =
  | 'updated'
  | 'conflict'
  | 'recoveryRequired'
  | 'failed'
  | 'skipped'

export async function performSessionWorkspaceWrite(
  conversationId: ConversationId
): Promise<SessionWorkspaceWriteResult> {
  const store = useSessionWorkspaceSyncStore.getState()
  if (
    !conversationId ||
    getActiveConversationId() !== conversationId ||
    store.activeConversationId !== conversationId ||
    store.isRestoreInProgress(conversationId) ||
    isTerminalRestoreInProgress() ||
    store.getConflict(conversationId)
  ) {
    return 'skipped'
  }
  const result = await sessionWorkspaceApi.writeWorkspace(
    conversationId,
    store.getBasedRevision(conversationId),
    buildSessionWorkspace(conversationId)
  )
  if (!result.success) {
    void logFrontendError({
      source: 'session-workspace-sync',
      message: `workspace write failed code=${result.code} conversationId=${conversationId}`
    })
    return 'failed'
  }
  if (result.data.status === 'updated') {
    store.setBasedRevision(conversationId, result.data.revision)
    return 'updated'
  }
  if (result.data.status === 'conflict') {
    store.setConflict(conversationId, {
      conversationId,
      currentRevision: result.data.currentRevision,
      currentUpdatedAtUtc: result.data.currentUpdatedAtUtc,
      currentUpdateIdentity: result.data.currentUpdateIdentity
    })
    return 'conflict'
  }
  store.setRecoveryItems(conversationId, result.data.recoveryItems)
  return 'recoveryRequired'
}

export async function resolveSessionWorkspaceConflict(
  conversationId: ConversationId,
  action: 'reload' | 'overwrite' | 'dismiss'
): Promise<void> {
  const store = useSessionWorkspaceSyncStore.getState()
  const conflict = store.getConflict(conversationId)
  if (!conflict) return
  if (action === 'reload') {
    store.setConflict(conversationId, null)
    await loadSessionWorkspace(conversationId)
    return
  }
  store.setBasedRevision(conversationId, conflict.currentRevision)
  store.setConflict(conversationId, null)
  if (action === 'overwrite') {
    const outcome = await performSessionWorkspaceWrite(conversationId)
    if (outcome === 'failed' || outcome === 'skipped') {
      store.setConflict(conversationId, conflict)
    }
  }
}

export async function resolveSessionWorkspaceRecovery(
  conversationId: ConversationId,
  item: RecoveryItemV1,
  action: RecoveryActionName
): Promise<void> {
  const common = { recoveryId: item.recoveryId, expectedRevision: item.revision }
  let request: ResolveRecoveryItemRequest
  switch (action) {
    case 'inspect':
      request = { ...common, action, payload: {} }
      break
    case 'associateConversation':
      request = {
        ...common,
        action,
        idempotencyKey: randomUUID(),
        payload: { conversationId }
      }
      break
    case 'startEmptyWorkspace':
      request = {
        ...common,
        action,
        idempotencyKey: randomUUID(),
        payload: {
          conversationId,
          expectedWorkspaceRevision: useSessionWorkspaceSyncStore
            .getState()
            .getBasedRevision(conversationId)
        }
      }
      break
    case 'dismissPreservedSource':
      request = {
        ...common,
        action,
        idempotencyKey: randomUUID(),
        payload: { reasonCode: 'deferLegacyProjection' }
      }
      break
  }
  const result = await sessionWorkspaceApi.resolveRecovery(request)
  if (!result.success) {
    void logFrontendError({
      source: 'session-workspace-sync',
      message: `recovery action failed code=${result.code} conversationId=${conversationId} recoveryId=${item.recoveryId} action=${action}`
    })
    return
  }
  const updated: RecoveryItemV1 = {
    ...item,
    status: result.data.status,
    revision: result.data.recoveryRevision
  }
  const store = useSessionWorkspaceSyncStore.getState()
  store.setRecoveryItems(
    conversationId,
    store
      .getRecoveryItems(conversationId)
      .map((existing) => (existing.recoveryId === item.recoveryId ? updated : existing))
  )
  if (result.data.workspaceChanged) await loadSessionWorkspace(conversationId)
}

export function useSessionWorkspaceBootstrap(): void {
  const conversationId = useConversationStore((state) => state.activeConversationId)
  useEffect(() => {
    useSessionWorkspaceSyncStore.getState().setActiveConversationId(conversationId)
  }, [conversationId])
}

export function useSessionWorkspaceSync(conversationId: ConversationId | null): void {
  useEffect(() => {
    if (!conversationId) return
    let timer: ReturnType<typeof setTimeout> | null = null
    const schedule = (): void => {
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        void performSessionWorkspaceWrite(conversationId)
      }, WRITE_DEBOUNCE_MS)
    }
    const unsubscribeWorkspace = useWorkspaceStore.subscribe((state, previous) => {
      if (state.root !== previous.root || state.activePaneId !== previous.activePaneId) schedule()
    })
    const unsubscribeEditor = useEditorStore.subscribe((state, previous) => {
      if (state.openFiles !== previous.openFiles) schedule()
    })
    const unsubscribeTerminal = useTerminalStore.subscribe((state, previous) => {
      if (state.terminals !== previous.terminals) schedule()
    })
    return () => {
      if (timer) clearTimeout(timer)
      unsubscribeWorkspace()
      unsubscribeEditor()
      unsubscribeTerminal()
    }
  }, [conversationId])
}
