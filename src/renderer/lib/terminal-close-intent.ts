import { isConversationScopedTerminal, type Terminal } from '@/types/project'

/**
 * Whether closing this terminal's tab leaves its process running.
 *
 * The single authority for that question. The close handler routes on it, and
 * both tab bars use it to label the × and to decide whether "Kill process" is
 * a distinct action or would just repeat what × already does — three copies of
 * the same condition would eventually disagree, and the way they'd disagree is
 * a button whose label promises the opposite of what it does.
 *
 * A terminal an agent opened inside a Conversation is one resource among many
 * the agent still owns, so closing its view keeps the process. A project shell
 * has no other owner: a close that left it running would leak it.
 */
export function closingKeepsProcessAlive(terminal: Pick<Terminal, 'conversationId'>): boolean {
  return isConversationScopedTerminal(terminal)
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
 * The two confirmations are separate settings, and that separation is the whole
 * point. `confirmViewClose` guards closing a *view*, whose dialog promises the
 * process keeps running; `confirmTerminate` guards ending the process, which
 * takes the shell and everything it started. Deriving one from the other would
 * let an opt-out given for the harmless action silence the prompt for the
 * unrecoverable one.
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
