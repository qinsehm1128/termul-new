import type { TerminalStatus } from '@shared/types/ipc.types'
import type { PersistedTerminalLayout } from '@shared/types/persistence.types'
import {
  isProjectTerminalRestoreInProgress,
  loadPersistedTerminals
} from '@/hooks/useTerminalAutoSave'
import { terminalApi } from '@/lib/api'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { findPaneContainingTab, terminalTabId, useWorkspaceStore } from '@/stores/workspace-store'

/**
 * A live PTY a project restore may take over: running, owned by this project
 * and by no Conversation workspace. An unscoped PTY (SSH, ad-hoc spawn) belongs
 * to no project, and another project's PTY must never be adopted.
 */
export function isAdoptableLiveTerminal(live: TerminalStatus, projectId: string): boolean {
  if (!live.active || live.workspaceRefTracked || live.conversationId) {
    return false
  }
  return live.projectId === projectId
}

/**
 * Take over every live PTY of a project the store does not hold yet, named as
 * the saved layout named it. Restore trusts the layout and the store; the PTYs
 * Terminal Core still runs are the ground truth both can lose track of.
 * Returns the ids of the records it added.
 */
export function adoptMissingProjectPtys(
  projectId: string,
  live: readonly TerminalStatus[],
  layout: PersistedTerminalLayout | null
): string[] {
  const adopted: string[] = []
  for (const terminal of live) {
    if (!isAdoptableLiveTerminal(terminal, projectId)) continue
    const store = useTerminalStore.getState()
    if (store.findTerminalByPtyId(terminal.id)) continue
    const id = store.adoptRemoteProjectTerminal({
      terminalId: terminal.id,
      projectId,
      cwd: terminal.cwd,
      cols: terminal.cols,
      rows: terminal.rows,
      shell: terminal.shell
    })
    if (!id) continue
    const saved = layout?.terminals.find((candidate) => candidate.ptyId === terminal.id)
    if (saved?.name) useTerminalStore.getState().renameTerminal(id, saved.name)
    adopted.push(id)
  }
  return adopted
}

export interface TerminalRecoveryResult {
  /** Live PTYs the renderer had lost and took back. */
  adopted: number
  /** Tabs put back into the active project's panes. */
  tabsRestored: number
  /** Records whose PTY Terminal Core no longer runs. */
  disconnected: number
}

/**
 * Take back the live PTYs of every known project the store has lost, project
 * by project. `skip` leaves out a project whose own restore is about to run or
 * is running, so the two never add the same PTY. Returns how many came back.
 */
export async function adoptLiveTerminalsOfAllProjects(
  live: readonly TerminalStatus[],
  skip: (projectId: string) => boolean = () => false
): Promise<number> {
  const known = new Set(useProjectStore.getState().projects.map((project) => project.id))
  const projectIds = new Set<string>()
  for (const terminal of live) {
    if (!terminal.projectId || !known.has(terminal.projectId)) continue
    if (!isAdoptableLiveTerminal(terminal, terminal.projectId)) continue
    if (useTerminalStore.getState().findTerminalByPtyId(terminal.id)) continue
    projectIds.add(terminal.projectId)
  }

  let adopted = 0
  for (const projectId of projectIds) {
    if (skip(projectId)) continue
    const layout = await loadPersistedTerminals(projectId)
    // The restore may have started while the layout loaded.
    if (skip(projectId)) continue
    adopted += adoptMissingProjectPtys(projectId, live, layout).length
  }
  return adopted
}

/**
 * Run once when the renderer starts: a reload empties the store while
 * Terminal Core keeps every terminal running, and only the active project gets
 * its terminals back from its own restore. The active project and any project
 * already restoring are left to that restore.
 */
export async function adoptLiveTerminalsOnBoot(): Promise<number> {
  if (typeof terminalApi.list !== 'function') return 0
  const listed = await terminalApi.list()
  if (!listed.success) throw new Error(listed.error)
  return adoptLiveTerminalsOfAllProjects(
    listed.data,
    (projectId) =>
      projectId === useProjectStore.getState().activeProjectId ||
      isProjectTerminalRestoreInProgress(projectId)
  )
}

/**
 * Rebuild the renderer's terminals from what Terminal Core is running, without
 * reloading the page or touching any process.
 *
 * Every project gets its running terminals back, not only the active one: a
 * renderer reload empties the store, and a project left out vanished from the
 * workspace while its terminals kept running. Only the active project's tabs
 * are put on screen; another project shows its tabs when it is opened.
 */
export async function recoverTerminalWorkspace(): Promise<TerminalRecoveryResult> {
  if (typeof terminalApi.list !== 'function') throw new Error('Terminal list unavailable')
  const listed = await terminalApi.list()
  if (!listed.success) throw new Error(listed.error)
  const live = listed.data
  const liveIds = new Set(live.filter((terminal) => terminal.active).map((terminal) => terminal.id))

  let disconnected = 0
  for (const terminal of useTerminalStore.getState().terminals) {
    if (!terminal.ptyId || liveIds.has(terminal.ptyId)) continue
    const status = terminal.healthStatus
    if (status === 'exited' || status === 'crashed' || status === 'disconnected') continue
    useTerminalStore.getState().setTerminalHealthStatus(terminal.id, 'disconnected')
    disconnected += 1
  }

  const adopted = await adoptLiveTerminalsOfAllProjects(live)
  const activeProjectId = useProjectStore.getState().activeProjectId

  let tabsRestored = 0
  if (activeProjectId) {
    for (const terminal of useTerminalStore.getState().terminals) {
      if (terminal.projectId !== activeProjectId || terminal.viewState === 'hidden') continue
      if (!terminal.ptyId || !liveIds.has(terminal.ptyId)) continue
      const workspace = useWorkspaceStore.getState()
      if (findPaneContainingTab(workspace.root, terminalTabId(terminal.id))) continue
      workspace.ensureTerminalTab(terminal.id, undefined, false)
      tabsRestored += 1
    }
  }

  return { adopted, tabsRestored, disconnected }
}
