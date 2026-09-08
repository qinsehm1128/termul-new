/**
 * The two status marks on a terminal tab.
 *
 * They are deliberately separate glyph families because they answer different
 * questions about different subjects, and a single mark carrying both was
 * unreadable: a dot that meant "output arrived" said nothing about whether the
 * agent inside had stopped to ask a question.
 *
 * - **Dot** — the terminal: running, producing output, or wanting attention.
 *   Applies to every terminal, agent or plain shell.
 * - **Star** — the agent inside it, if any and if we can tell.
 */

import { useTranslation } from 'react-i18next'
import type { AgentTerminalState } from '@/lib/agents/agent-terminal-state'
import { cn } from '@/lib/utils'

/**
 * The terminal's own state. Highest-urgency condition wins; nothing is drawn for
 * a terminal that is neither running nor recently active.
 */
export function TabRunMark({
  attention,
  activity,
  running
}: {
  attention?: boolean
  activity?: boolean
  running?: boolean
}): React.JSX.Element | null {
  if (attention) {
    return (
      <span
        data-run-mark="attention"
        className="mr-0.5 size-1.5 shrink-0 rounded-full bg-warning"
        aria-hidden
      />
    )
  }
  if (activity) {
    return (
      <span
        data-run-mark="activity"
        className="mr-0.5 size-1.5 shrink-0 rounded-full bg-primary"
        aria-hidden
      />
    )
  }
  if (running) {
    return (
      <span
        data-run-mark="running"
        className="mr-0.5 size-1.5 shrink-0 rounded-full bg-primary/40"
        aria-hidden
      />
    )
  }
  return null
}

/**
 * What the agent inside the terminal is doing.
 *
 * A star, borrowing the agent's own vocabulary — claude writes `✳` into its
 * title once a turn settles.
 *
 * Nothing is drawn for `unknown`. Absence is the honest rendering of "no
 * evidence": a dimmed star would be read as idle, which is exactly the wrong
 * answer for an agent that might be sitting on a permission prompt we cannot
 * see. State is carried by colour *and* motion so neither a monochrome display
 * nor `prefers-reduced-motion` leaves the three states indistinguishable, and
 * the label is exposed to assistive tech since the glyph alone says little.
 */
export function TabAgentMark({ state }: { state?: AgentTerminalState }): React.JSX.Element | null {
  const { t } = useTranslation('terminal')
  if (state == null || state === 'unknown') return null

  const label = t(`agentState.${state}`)
  return (
    <span
      role="status"
      aria-label={label}
      title={label}
      data-agent-state={state}
      className={cn(
        'mr-1 shrink-0 text-[9px] leading-none',
        state === 'blocked' && 'text-warning',
        state === 'working' && 'text-primary motion-safe:animate-pulse',
        state === 'idle' && 'text-muted-foreground/60'
      )}
    >
      ✳
    </span>
  )
}
