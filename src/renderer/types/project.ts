// Import GitStatus from shared types to ensure consistency
// between IPC contract and renderer domain models
import { acceptedBrandValues } from '@shared/brand'
import type { GitStatus, TerminalModes } from '@shared/types/ipc.types'
import type { TerminalResourceHydrationStatus } from '@shared/types/session-workspace.types'
import type { AgentTerminalState } from '@/lib/agents/agent-terminal-state'

// Re-export for convenience
export type { GitStatus, TerminalModes }

export type ProjectColor =
  | 'blue'
  | 'purple'
  | 'green'
  | 'yellow'
  | 'red'
  | 'cyan'
  | 'pink'
  | 'orange'
  | 'gray'

export interface Worktree {
  id: string
  name: string
  branch: string
  path: string
  createdAt: string // ISO timestamp
}

export interface ProjectGroup {
  id: string
  name: string
  projectIds: string[]
  preferredProjectId?: string
  isCollapsed?: boolean
  color?: ProjectColor
}

export interface Project {
  id: string
  name: string
  color: ProjectColor
  path?: string
  isActive?: boolean
  /**
   * `true` when this is the host's default project (set by the host's
   * `default_project_id`). Mirrors `ProjectSummary.isDefault` on the wire.
   * Distinct from `isActive` (per-client, set locally by `selectProject`).
   * Surfaced in the desktop-hosted shared-live bridge (`useProjectsAutoSave`
   * maps `summary.isDefault` → `Project.isDefault`).
   */
  isDefault?: boolean
  isArchived?: boolean
  gitBranch?: string
  lastOpened?: Date
  defaultShell?: string
  envVars?: EnvVariable[]
  worktrees?: Worktree[]
  activeWorktreeId?: string | null
  isGitRepo?: boolean
  symlinkDirs?: string[] // Directories to symlink from project root into worktrees
}

// Helper getters for worktree operations
export function getActiveWorktree(project: Project): Worktree | undefined {
  if (!project.activeWorktreeId) return undefined
  return project.worktrees?.find((w) => w.id === project.activeWorktreeId)
}

/**
 * `Pick<…, 'path'>` rather than a full `Worktree`: the reconciler asks the same
 * question of a raw `GitWorktreeEntry` straight from `git worktree list`, and
 * one shared rule is the point — a second copy at that call site is how the two
 * drift apart at the next flip.
 */
export function isWorktreeSeManaged(worktree: Pick<Worktree, 'path'>): boolean {
  // Normalize path separators for cross-platform detection
  const normalizedPath = worktree.path.replace(/\\/g, '/')
  // Every workspace dir this app has ever written, most-current first: a
  // worktree created before T-A18 still sits under the legacy one, and one
  // created after sits under the canonical one. Naming either alone
  // misclassifies half the worktrees on disk.
  return acceptedBrandValues('workspaceDir').some((dir) =>
    normalizedPath.includes(`${dir}/worktrees/`)
  )
}

/**
 * `exited` is a clean shell exit (status 0); `crashed` is a non-zero or
 * signal-killed one. They are separate because the UI treats them differently:
 * a clean exit is the expected end of a session, not a project-level error.
 */
export type TerminalHealthStatus =
  | TerminalResourceHydrationStatus
  | 'exited'
  | 'crashed'
  | 'hibernated'
export type TerminalViewState = 'visible' | 'hidden' | 'detached'

export interface Terminal {
  id: string
  /** Conversation ownership scope; absent for scope-less project terminals. */
  conversationId?: string
  ptyId?: string
  name: string
  /** Optional attribution/filter only; never ownership or authorization. */
  projectId?: string
  shell: string
  cwd?: string
  worktreeId?: string
  gitBranch?: string | null
  gitStatus?: GitStatus | null
  lastExitCode?: number | null
  isActive?: boolean
  output?: TerminalLine[]
  pendingScrollback?: string[] // Legacy text snapshot to restore on terminal mount
  transcript?: string // Raw PTY transcript used for ANSI/styling-preserving restoration
  /**
   * Whether the transcript has lost its oldest bytes to a size cap. A trim can
   * take a DEC mode transition with it, and the cached-remount replay writes
   * the transcript raw onto a live instance, so it has no heuristic to fall
   * back on. Surfaced through the replay telemetry rather than acted on.
   */
  transcriptTrimmed?: boolean
  /**
   * Captured DEC private-mode snapshot (R3) to replay before `pendingScrollback`
   * on terminal mount, so an alt-screen TUI (vim/tmux/less) restores its
   * screen/modes. Optional — absence degrades to content-only restore.
   */
  pendingModes?: TerminalModes
  detachedOutput?: string // Raw PTY output captured while no renderer is mounted
  rendererAttachmentCount?: number // Number of mounted renderers bound to this PTY
  healthStatus?: TerminalHealthStatus // Terminal health status
  /** Latest host replay watermark retained only for renderer-side resume retries. */
  resumeCursor?: number
  /** Explicit view lifecycle, independent from the live PTY resource. */
  viewState?: TerminalViewState
  isHidden?: boolean // Compatibility mirror of viewState === 'hidden' | 'detached'
  hiddenSince?: number // Timestamp when terminal became hidden within the workspace/pane model
  isAppHidden?: boolean // Whether the entire app/window is currently hidden or minimized
  appHiddenSince?: number // Timestamp when the app-hidden retention window started
  hasActivity?: boolean // Whether terminal has recent output activity
  lastActivityTimestamp?: number // Timestamp when last activity occurred
  needsAttention?: boolean // Whether this terminal's process finished while it was not the focused/visible terminal; drives the in-app highlight border
  // ADR-004.4: terminal-native agent launch metadata. Descriptive-only — no
  // behavior keys off these except tab labeling and restore-prompt suppression.
  // Git/cwd trackers, resize, and persistence ignore them.
  agentId?: string // Agent Registry id, e.g. 'claude-code'
  agentName?: string // Display name, e.g. 'Claude Code' — used for the tab label
  agentProgram?: string // Resolved/declared program for restore re-spawn (no prompt)
  agentArgs?: string[] // baseArgs only (seed prompt intentionally excluded for restore)
  kind?: 'shell' | 'agent' // Session type marker; defaults to 'shell' when unset
  /**
   * Latest OSC 0/2 title the child process set, or null once it cleared it.
   * Evidence for `agentState`, not a display label — the tab keeps using
   * `agentName`/`name`. In-memory only; auto-save ignores it.
   */
  oscTitle?: string | null
  /**
   * What the agent in this terminal is doing, derived from the OSC title and
   * the screen tail. `'unknown'` means no usable evidence, which is deliberately
   * distinct from `'idle'`. In-memory only; auto-save ignores it.
   */
  agentState?: AgentTerminalState
  /**
   * CAP-3: the reclaimable-terminal lease credential issued at spawn.
   * IN-MEMORY ONLY — never written to auto-save/snapshot persistence,
   * localStorage, or any browser storage. Set on spawn/rotate, cleared on
   * kill/close/restart/clearTerminalPtyId.
   */
  claim?: string
}

/** True when the renderer should keep a workspace tab for this record. */
export function isOpenTerminalView(terminal: Pick<Terminal, 'viewState' | 'isHidden'>): boolean {
  return terminal.viewState !== 'hidden' && !terminal.isHidden
}

/** Conversation-owned PTY: hide can keep the process. Project shells should die on close. */
export function isConversationScopedTerminal(terminal: Pick<Terminal, 'conversationId'>): boolean {
  return Boolean(terminal.conversationId)
}

/**
 * A terminal that belongs to a project — the *only* definition of that.
 *
 * `projectId` alone is not it. A Conversation's terminal also carries a project
 * id, for attribution: it is how the terminal is labelled and grouped, not who
 * owns it. Treating "has this project id" as "is a project terminal" is what
 * let the two kinds mix — Conversation terminals counted in project totals and
 * materialised into the project's tab bar, and project shells pulled into a
 * Conversation's workspace.
 *
 * They are separate things and answer to separate owners: a project terminal
 * lives in the project layout, a Conversation terminal lives in that
 * Conversation's SessionWorkspace. Every project-scoped surface asks this
 * predicate so the two can never drift apart again.
 */
export function isProjectScopedTerminal(
  terminal: Pick<Terminal, 'projectId' | 'conversationId'>,
  projectId: string
): boolean {
  return terminal.projectId === projectId && !isConversationScopedTerminal(terminal)
}

/** Live PTY whose view is closed, scoped to the current Conversation or project shell. */
export function isHiddenRunningTerminal(
  terminal: Pick<Terminal, 'ptyId' | 'viewState' | 'isHidden' | 'conversationId' | 'projectId'>,
  scope: { conversationId?: string | null; projectId?: string | null }
): boolean {
  if (!terminal.ptyId || isOpenTerminalView(terminal)) return false
  if (scope.conversationId && terminal.conversationId === scope.conversationId) return true
  return (
    Boolean(scope.projectId) &&
    terminal.projectId === scope.projectId &&
    !isConversationScopedTerminal(terminal)
  )
}

export interface TerminalLine {
  type: 'command' | 'output' | 'error' | 'warning' | 'info' | 'success'
  content: string
}

export interface Snapshot {
  id: string
  projectId: string
  name: string
  description?: string
  createdAt: Date
  paneCount: number
  processCount: number
  tag?: 'stable' | 'base'
  thumbnail?: SnapshotThumbnail
}

export interface SnapshotThumbnail {
  layout: 'single' | 'split-v' | 'split-h' | 'grid'
  lines: { color: string; width: number }[]
}

export interface EnvVariable {
  key: string
  value: string
  isSecret?: boolean
}
