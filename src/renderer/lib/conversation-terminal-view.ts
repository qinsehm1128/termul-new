/**
 * The invariant for a terminal-backed Conversation: opening it shows a live
 * terminal in the folder the Conversation was created for.
 *
 * This lives in activation rather than in the launcher, and that placement is
 * the whole point. A terminal spawned by the launcher lands in the workspace
 * that is on screen *at that moment* — which activation then replaces with the
 * Conversation's own workspace, orphaning the tab in the project the user was
 * looking at. Agent chats never had this problem because their tab is created
 * by activation, after the swap. Terminals now follow the same rule.
 *
 * Because the rule is "activating guarantees a terminal", first launch and
 * every later reopen are the same code path — there is no separate restore
 * branch that can drift from the create branch.
 *
 * Three states, in order:
 *
 * 1. a terminal of this Conversation is already on screen — nothing to do;
 * 2. one is alive but its view was closed — reattach the view, never respawn,
 *    because the process holds the user's scrollback and running commands;
 * 3. none is alive — spawn one in the Conversation's folder and record it.
 */

import type { ConversationRecordV2 } from '@shared/types/conversation.types'
import { conversationBackendOf } from '@shared/types/conversation.types'
import { toast } from 'sonner'
import { runtimeT } from '@/i18n/runtime'
import { conversationApi } from '@/lib/conversation-api'
import { logFrontendError } from '@/lib/log-api'
import { spawnTerminalInPane } from '@/lib/terminal-spawn'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useTerminalStore } from '@/stores/terminal-store'
import { getAllLeafPanes, useWorkspaceStore } from '@/stores/workspace-store'
import { isConversationScopedTerminal, isOpenTerminalView, type Terminal } from '@/types/project'

/**
 * The directory a terminal-backed Conversation runs in: its own Conversation
 * folder, whatever the execution target is.
 *
 * Same folder the host calls `executionCwd` — `resolve_execution_scope` in
 * `src-tauri/src/conversation/creation.rs` returns the workspace path for every
 * target, and the target only widens which *other* roots are reachable. Reading
 * it off the record means reopening needs no `prepare` round-trip.
 *
 * This briefly returned the project root for `project_root` targets, on the
 * reasoning that an empty UUID-named folder is useless to a shell. That was
 * wrong and the correction is worth keeping written down: the folder is what
 * makes a terminal Conversation a Conversation. A shell that opens in the
 * project root is indistinguishable from an ordinary project terminal, and
 * reopening one no longer returns the user to where that session's work was.
 * The execution target still decides project attribution — see
 * {@link conversationProjectId} — just not the directory.
 */
export function conversationTerminalCwd(
  record: Pick<ConversationRecordV2, 'workspaceCwd'> | undefined
): string {
  return record?.workspaceCwd ?? ''
}

/**
 * True when this terminal *is* its Conversation's backend, rather than a shell
 * an agent opened inside one.
 *
 * The distinction decides what closing the tab means. An agent's terminal is
 * one resource among many in a Conversation the agent still owns, so closing
 * its view has to leave the process running. A terminal-backed Conversation has
 * nothing else in it — "close the view, keep the process" gives the user a live
 * shell with no tab, no process list, and no way back to it, which is exactly
 * the state that made terminals here feel unkillable.
 */
export function isConversationBackendTerminal(terminal: Pick<Terminal, 'conversationId'>): boolean {
  if (!terminal.conversationId) return false
  const store = useConversationStore.getState()
  const record =
    store.summariesById[terminal.conversationId] ??
    store.detailsById[terminal.conversationId]?.conversation
  return conversationBackendOf(record) === 'terminal'
}

/**
 * Whether closing this terminal's tab leaves its process running.
 *
 * The single authority for that question. The close handler routes on it, and
 * both tab bars use it to label the × and to decide whether "Kill process" is
 * a distinct action or would just repeat what × already does — three copies of
 * the same condition would eventually disagree, and the way they'd disagree is
 * a button whose label promises the opposite of what it does.
 */
export function closingKeepsProcessAlive(terminal: Pick<Terminal, 'conversationId'>): boolean {
  return isConversationScopedTerminal(terminal) && !isConversationBackendTerminal(terminal)
}

/** What clicking × on a terminal tab should do. */
export type TerminalCloseIntent =
  | 'terminate-confirm'
  | 'terminate'
  | 'close-view-confirm'
  | 'close-view'

/**
 * Decide what × means for one terminal.
 *
 * A function rather than a branch inside the close handler because the rule is
 * worth proving and the handler lives in a component whose pane tree is not
 * reachable from a test.
 *
 * The two confirmations are separate settings, and that separation is the whole
 * point. `confirmViewClose` guards closing a *view*, whose dialog promises the
 * process keeps running; `confirmTerminate` guards ending the process, which
 * takes the shell and — through the host's session sweep — everything it
 * started. Deriving one from the other would let an opt-out given for the
 * harmless action silence the prompt for the unrecoverable one. A user who
 * wants no prompt at all can still say so, but has to say it about the kill.
 */
export function terminalCloseIntent(
  terminal: Pick<Terminal, 'conversationId'> | undefined,
  confirmViewClose: boolean,
  confirmTerminate: boolean
): TerminalCloseIntent {
  if (!terminal || !closingKeepsProcessAlive(terminal)) {
    return confirmTerminate ? 'terminate-confirm' : 'terminate'
  }
  return confirmViewClose ? 'close-view-confirm' : 'close-view'
}

/** Project attribution for the spawned terminal; never an ownership key. */
function conversationProjectId(
  record: Pick<ConversationRecordV2, 'executionTarget' | 'projectAttachment'> | undefined
): string {
  const target = record?.executionTarget
  if (target?.kind === 'project_root' || target?.kind === 'worktree') return target.projectId
  return record?.projectAttachment?.projectId ?? ''
}

/**
 * A record that can still be reattached rather than replaced.
 *
 * `disconnected` counts: the resume round-trip in `reconcileTerminalResources`
 * retires records whose PTY is genuinely gone, so a record that survived it
 * with a `ptyId` is one the host still knows about.
 */
function isReattachable(terminal: Terminal): boolean {
  if (!terminal.ptyId) return false
  return terminal.healthStatus !== 'exited' && terminal.healthStatus !== 'crashed'
}

function hasVisibleTab(terminalId: string): boolean {
  return getAllLeafPanes(useWorkspaceStore.getState().root).some((leaf) =>
    leaf.tabs.some((tab) => tab.type === 'terminal' && tab.terminalId === terminalId)
  )
}

export type EnsureConversationTerminalOutcome =
  | 'already-open'
  | 'reattached'
  | 'spawned'
  | 'stale'
  | 'no-pane'
  | 'failed'

/**
 * Bring the Conversation's terminal on screen, spawning one if none survives.
 *
 * `isCurrent` is the activation-staleness guard: the user can click a second
 * Conversation while this one is still resolving, and a terminal spawned after
 * that would appear in the wrong workspace — the very bug this function exists
 * to prevent.
 */
export async function ensureConversationTerminal(
  conversationId: string,
  isCurrent: () => boolean = () => true
): Promise<EnsureConversationTerminalOutcome> {
  if (!isCurrent()) return 'stale'

  const owned = useTerminalStore
    .getState()
    .terminals.filter((terminal) => terminal.conversationId === conversationId)

  if (owned.some((terminal) => isOpenTerminalView(terminal) && hasVisibleTab(terminal.id))) {
    return 'already-open'
  }

  const reattachable = owned.find(isReattachable)
  if (reattachable) {
    useWorkspaceStore.getState().reopenTerminalView(reattachable.id)
    return 'reattached'
  }

  const conversationStore = useConversationStore.getState()
  const record =
    conversationStore.summariesById[conversationId] ??
    conversationStore.detailsById[conversationId]?.conversation
  const cwd = conversationTerminalCwd(record)
  if (!cwd) {
    void logFrontendError({
      source: 'conversation-terminal-view.ensure',
      message: `conversationId=${conversationId} has no directory to open a terminal in`
    })
    return 'failed'
  }

  const paneId =
    useWorkspaceStore.getState().activePaneId ??
    getAllLeafPanes(useWorkspaceStore.getState().root)[0]?.id
  if (!paneId) return 'no-pane'

  const spawned = await spawnTerminalInPane(paneId, conversationProjectId(record), cwd, {
    conversationId,
    maxTerminalsPerProject: useAppSettingsStore.getState().settings.maxTerminalsPerProject
  })
  if (!isCurrent()) return 'stale'
  if (!spawned.success) {
    // Silent failure here reads as "the Conversation is empty", which is the
    // one thing the user cannot act on. The limit message in particular is
    // actionable only if they see it.
    toast.error(
      spawned.error ?? runtimeT('terminal', 'errors.createFailed', 'Failed to create terminal')
    )
    return 'failed'
  }

  const provisioned = await conversationApi.provisionTerminalConversation(
    conversationId,
    spawned.terminalId
  )
  if (!provisioned.success) {
    // The terminal is real and on screen. Only the durable "this terminal backs
    // this Conversation" record is missing, and killing a working shell to tidy
    // that up would destroy what the user can already see.
    void logFrontendError({
      source: 'conversation-terminal-view.provision',
      message: `conversationId=${conversationId} terminalId=${spawned.terminalId} provision failed: ${provisioned.error ?? 'unknown'}`
    })
  }

  // Spawning registered the terminal as a Conversation resource host-side,
  // which bumped the workspace revision. The renderer is still holding the
  // revision it loaded a moment ago, so its next auto-save would be rejected as
  // a conflict and the user would be told their workspace "changed elsewhere" —
  // about a write their own click caused.
  const workspaceModule = await import('@/hooks/use-session-workspace-sync')
  if (!isCurrent()) return 'spawned'
  await workspaceModule.adoptHostWorkspaceRevision(conversationId)
  return 'spawned'
}
