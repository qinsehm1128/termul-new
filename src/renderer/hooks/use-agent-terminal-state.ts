/**
 * Keeps every agent terminal's `agentState` current.
 *
 * Two inputs feed it. The OSC title arrives as a host event and covers terminals
 * that have never been rendered. The screen tail has to be polled, because
 * xterm has no "buffer changed" event — but polling is gated on output, so an
 * idle workspace does no work at all.
 *
 * This runs across every terminal, so treat it as a multiplicative path: cost is
 * per tick × agent terminals. Shell terminals and agents with no rules are
 * filtered out before any buffer is touched.
 */

import { useEffect } from 'react'
import { readTerminalScreenTail } from '@/lib/agents/agent-screen-source'
import {
  type AgentStateEvidence,
  deriveAgentTerminalState,
  isStateAwareAgent
} from '@/lib/agents/agent-terminal-state'
import { terminalApi } from '@/lib/api'
import { useTerminalStore } from '@/stores/terminal-store'
import type { Terminal } from '@/types/project'

/** How often the screen tail is sampled while a terminal is producing output. */
const SCAN_INTERVAL_MS = 600

/**
 * Extra passes after output stops.
 *
 * xterm schedules `write()` on timers/microtasks, so the buffer can still be
 * behind the activity timestamp that triggered a scan. Without a trailing pass
 * the final frame of a turn — the one that says the agent went idle or is now
 * asking a question — can be missed until the next output, which may never come.
 */
const SETTLE_PASSES = 2

export interface ScanMemory {
  /** `lastActivityTimestamp` observed at the last scan. */
  activityAt: number | undefined
  /** Trailing passes still owed after the most recent fresh output. */
  settleRemaining: number
}

/**
 * Whether this terminal needs its screen read on this tick.
 *
 * Returns the decision plus the memory to carry forward, so the caller never
 * has to reason about when to decrement the settle counter.
 */
export function planTerminalScan(
  terminal: Pick<Terminal, 'lastActivityTimestamp'>,
  memory: ScanMemory | undefined
): { scan: boolean; next: ScanMemory } {
  const activityAt = terminal.lastActivityTimestamp

  if (memory === undefined) {
    // First sight of this terminal: read it once so a long-idle agent still
    // reports a state instead of sitting at unknown forever.
    return { scan: true, next: { activityAt, settleRemaining: SETTLE_PASSES } }
  }

  if (activityAt !== memory.activityAt) {
    return { scan: true, next: { activityAt, settleRemaining: SETTLE_PASSES } }
  }

  if (memory.settleRemaining > 0) {
    return { scan: true, next: { activityAt, settleRemaining: memory.settleRemaining - 1 } }
  }

  return { scan: false, next: memory }
}

/** Terminals this hook is responsible for. */
export function isAgentStateCandidate(terminal: Terminal): boolean {
  return terminal.kind === 'agent' && isStateAwareAgent(terminal.agentId)
}

export function useAgentTerminalState(): void {
  const setTerminalOscTitle = useTerminalStore((state) => state.setTerminalOscTitle)
  const setTerminalAgentState = useTerminalStore((state) => state.setTerminalAgentState)
  const findTerminalByPtyId = useTerminalStore((state) => state.findTerminalByPtyId)

  useEffect(() => {
    return terminalApi.onOscTitleChanged((ptyId: string, title: string | null) => {
      const terminal = findTerminalByPtyId(ptyId)
      if (terminal) setTerminalOscTitle(terminal.id, title)
    })
  }, [findTerminalByPtyId, setTerminalOscTitle])

  useEffect(() => {
    const memories = new Map<string, ScanMemory>()

    const tick = (): void => {
      const terminals = useTerminalStore.getState().terminals
      const live = new Set<string>()

      for (const terminal of terminals) {
        if (!isAgentStateCandidate(terminal)) continue
        live.add(terminal.id)

        const { scan, next } = planTerminalScan(terminal, memories.get(terminal.id))
        memories.set(terminal.id, next)
        if (!scan) continue

        const evidence: AgentStateEvidence = {
          // `isAgentStateCandidate` already established this is set.
          agentId: terminal.agentId as string,
          oscTitle: terminal.oscTitle ?? null,
          screenTail: readTerminalScreenTail(terminal.id, terminal.ptyId)
        }
        setTerminalAgentState(terminal.id, deriveAgentTerminalState(evidence).state)
      }

      // Closed terminals must not keep their scan memory: an id can come back
      // on restore, and a stale activity timestamp would suppress its first scan.
      for (const id of memories.keys()) {
        if (!live.has(id)) memories.delete(id)
      }
    }

    tick()
    const timer = setInterval(tick, SCAN_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [setTerminalAgentState])
}
