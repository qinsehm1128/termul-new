import { useProjectStore } from '@/stores/project-store'
import { useSSHStore } from '@/stores/ssh-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'
import { isOpenTerminalView } from '@/types/project'
import { logFrontendError } from './log-api'

export interface PendingTerminalFocus {
  projectId: string
  terminalId: string
}

/** Keep a board pick long enough for project restore to finish and re-apply it. */
const PENDING_FOCUS_TTL_MS = 2500

let pending: PendingTerminalFocus | null = null
let pendingExpireTimer: ReturnType<typeof setTimeout> | null = null

function armPendingExpire(): void {
  if (pendingExpireTimer !== null) {
    clearTimeout(pendingExpireTimer)
  }
  pendingExpireTimer = setTimeout(() => {
    pending = null
    pendingExpireTimer = null
  }, PENDING_FOCUS_TTL_MS)
}

export function setPendingTerminalFocus(next: PendingTerminalFocus): void {
  pending = next
  armPendingExpire()
}

export function peekPendingTerminalFocus(): PendingTerminalFocus | null {
  return pending
}

export function clearPendingTerminalFocus(): void {
  pending = null
  if (pendingExpireTimer !== null) {
    clearTimeout(pendingExpireTimer)
    pendingExpireTimer = null
  }
}

export function focusTerminalInWorkspace(terminalId: string): boolean {
  const terminal = useTerminalStore.getState().terminals.find((item) => item.id === terminalId)
  if (!terminal) return false
  const workspace = useWorkspaceStore.getState()
  if (terminal.ptyId && !isOpenTerminalView(terminal)) {
    workspace.reopenTerminalView(terminalId)
  } else {
    workspace.addTerminalTab(terminalId)
  }
  const root = useWorkspaceStore.getState().root
  return getAllLeafPanes(root).some((leaf) =>
    leaf.tabs.some((tab) => tab.type === 'terminal' && tab.terminalId === terminalId)
  )
}

/** Reveal a pending board pick after the project workspace is on screen. */
export function applyPendingTerminalFocus(projectId: string): boolean {
  if (!pending || pending.projectId !== projectId) return false
  return focusTerminalInWorkspace(pending.terminalId)
}

export function openBoardTerminal(options: {
  projectId: string | null
  terminalId: string
  navigate: (path: string) => void
}): void {
  const projectId = options.projectId?.trim() || null
  void logFrontendError({
    level: 'warn',
    source: 'terminal-board.open',
    message: projectId ? `open project terminal projectId=${projectId}` : 'open unassigned terminal'
  })
  useWorkspaceStore.getState().hideAgentLauncher()
  if (!projectId) {
    clearPendingTerminalFocus()
    options.navigate('/')
    focusTerminalInWorkspace(options.terminalId)
    return
  }
  setPendingTerminalFocus({ projectId, terminalId: options.terminalId })
  useProjectStore.getState().selectProject(projectId)
  useSSHStore.getState().selectProfile(null)
  options.navigate('/')
}

export function openBoardProject(options: {
  projectId: string
  navigate: (path: string) => void
}): void {
  void logFrontendError({
    level: 'warn',
    source: 'terminal-board.open-project',
    message: `open project projectId=${options.projectId}`
  })
  clearPendingTerminalFocus()
  useWorkspaceStore.getState().hideAgentLauncher()
  useProjectStore.getState().selectProject(options.projectId)
  useSSHStore.getState().selectProfile(null)
  options.navigate('/')
}
