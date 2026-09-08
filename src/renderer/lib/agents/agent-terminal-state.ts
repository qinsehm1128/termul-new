/**
 * Derives what a CLI agent running in a terminal is currently doing.
 *
 * Two evidence sources, in order of trust:
 *
 * - **OSC title** — captured host-side by `OscTitleTracker`, available for every
 *   terminal whether or not it has ever been rendered. Only `claude-code` and
 *   `codex` report status this way, but when they do it is unambiguous, so those
 *   rules carry the highest priority.
 * - **Screen tail** — the bottom of the xterm buffer. Covers the remaining
 *   agents and supplies the blocked states that no agent puts in its title.
 *   Only available once a terminal has been rendered at least once (see
 *   `agent-screen-source.ts`).
 *
 * The match strings are observable output of third-party CLIs. They were
 * cross-checked against herdr's published agent-detection manifests
 * (github.com/herdrdev/herdr, Apache-2.0) and re-expressed here; nothing is
 * copied from that project. They will drift as those CLIs change their UI —
 * treat a stale rule as expected maintenance, not a design failure.
 */

export type AgentTerminalState = 'working' | 'blocked' | 'idle' | 'unknown'

/** Which slice of evidence a rule reads. */
export type AgentRuleRegion =
  | { kind: 'oscTitle' }
  /** The whole captured screen tail (roughly one screenful). */
  | { kind: 'screen' }
  /** The bottom `lines` non-empty lines of the screen tail. */
  | { kind: 'screenBottom'; lines: number }

export interface AgentRule {
  /** Stable id, used in tests and when reporting why a state was chosen. */
  id: string
  state: Exclude<AgentTerminalState, 'unknown'>
  /** Highest match wins. Ties keep the earlier rule. */
  priority: number
  region: AgentRuleRegion
  test: (text: string) => boolean
}

export interface AgentStateEvidence {
  /** Registry id, e.g. `claude-code`. */
  agentId: string
  /** Latest OSC 0/2 title, or `null` if the child never set one. */
  oscTitle: string | null
  /**
   * Bottom-of-buffer text, or `null` when this terminal has no live xterm to
   * read. `null` is not the same as `''`: an empty screen is evidence, a
   * missing one is not.
   */
  screenTail: string | null
}

/** Claude's busy spinner: braille cells on older builds, half-circles on newer. */
const CLAUDE_BUSY_TITLE = /^[⠀-⣿◐-◓] /
/** Claude's settled marker, U+2733 EIGHT SPOKED ASTERISK. */
const CLAUDE_SETTLED_TITLE = /^✳ /
/** Codex's braille spinner, as a standalone token in the title. */
const CODEX_BUSY_TITLE = /(?:^| )[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏](?: |$)/
/** Cursor's spinner line: a glyph followed by a "…ing" verb. */
const CURSOR_BUSY_LINE = /^\s*(?:⬡|⬢|[⠀-⣿]+)\s+\p{Alphabetic}+\w*ing\b/mu

function includesAll(text: string, needles: string[]): boolean {
  const lower = text.toLowerCase()
  return needles.every((needle) => lower.includes(needle))
}

function includesAny(text: string, needles: string[]): boolean {
  const lower = text.toLowerCase()
  return needles.some((needle) => lower.includes(needle))
}

/**
 * Rules per registry agent id. Priorities follow one scale across sources so a
 * title rule and a screen rule can be compared directly:
 *
 * - 1000+ — the agent said so itself in its title
 * - 300-999 — a blocking prompt is visibly on screen
 * - 90-299 — a working hint is visibly on screen, or a title says "settled"
 */
const AGENT_RULES: Record<string, AgentRule[]> = {
  'claude-code': [
    {
      id: 'title-busy',
      state: 'working',
      priority: 1100,
      region: { kind: 'oscTitle' },
      test: (text) => CLAUDE_BUSY_TITLE.test(text)
    },
    {
      id: 'permission-prompt',
      state: 'blocked',
      priority: 980,
      region: { kind: 'screenBottom', lines: 12 },
      test: (text) =>
        includesAll(text, ['esc to cancel']) &&
        includesAny(text, [
          'do you want to proceed?',
          'enter to confirm',
          'tab to amend',
          'ctrl+e to explain'
        ])
    },
    {
      id: 'waiting-for-permission',
      state: 'blocked',
      priority: 960,
      region: { kind: 'screen' },
      test: (text) =>
        includesAny(text, ['waiting for permission', 'do you want to allow this connection?'])
    },
    {
      id: 'interrupt-hint',
      state: 'working',
      priority: 300,
      region: { kind: 'screenBottom', lines: 12 },
      test: (text) => includesAll(text, ['esc to interrupt'])
    },
    {
      id: 'title-settled',
      state: 'idle',
      priority: 250,
      region: { kind: 'oscTitle' },
      test: (text) => CLAUDE_SETTLED_TITLE.test(text)
    }
  ],
  codex: [
    {
      id: 'title-action-required',
      state: 'blocked',
      priority: 1100,
      region: { kind: 'oscTitle' },
      test: (text) => includesAll(text, ['action required'])
    },
    {
      id: 'title-busy',
      state: 'working',
      priority: 1050,
      region: { kind: 'oscTitle' },
      test: (text) => CODEX_BUSY_TITLE.test(text)
    },
    {
      // A title that is present but shows neither marker means codex has
      // settled — it keeps the title current, so this is real evidence rather
      // than an absence of it.
      id: 'title-settled',
      state: 'idle',
      priority: 100,
      region: { kind: 'oscTitle' },
      test: (text) =>
        text.trim().length > 0 &&
        !CODEX_BUSY_TITLE.test(text) &&
        !includesAll(text, ['action required'])
    }
  ],
  cursor: [
    {
      id: 'write-file-approval',
      state: 'blocked',
      priority: 320,
      region: { kind: 'screenBottom', lines: 8 },
      test: (text) =>
        includesAll(text, ['write to this file?']) &&
        includesAny(text, ['proceed (y)', 'reject & propose changes', 'esc or n or p'])
    },
    {
      id: 'approval-prompt',
      state: 'blocked',
      priority: 300,
      region: { kind: 'screen' },
      test: (text) =>
        includesAny(text, [
          'waiting for approval',
          'run (once) (y)',
          'skip (esc or n)',
          '(y) (enter)',
          'keep (n)'
        ])
    },
    {
      id: 'stop-hint',
      state: 'working',
      priority: 100,
      region: { kind: 'screenBottom', lines: 6 },
      test: (text) => includesAll(text, ['ctrl+c to stop'])
    },
    {
      id: 'spinner-line',
      state: 'working',
      priority: 90,
      region: { kind: 'screenBottom', lines: 8 },
      test: (text) => CURSOR_BUSY_LINE.test(text)
    }
  ],
  'gemini-cli': [
    {
      id: 'confirmation-prompt',
      state: 'blocked',
      priority: 300,
      region: { kind: 'screen' },
      test: (text) =>
        includesAny(text, [
          'apply this change',
          'allow execution',
          'waiting for user confirmation',
          'do you want to proceed'
        ])
    },
    {
      id: 'cancel-hint',
      state: 'working',
      priority: 100,
      region: { kind: 'screen' },
      test: (text) => includesAll(text, ['esc to cancel'])
    }
  ],
  opencode: [
    {
      id: 'permission-required',
      state: 'blocked',
      priority: 300,
      region: { kind: 'screen' },
      test: (text) => includesAll(text, ['permission required'])
    },
    {
      id: 'interrupt-hint',
      state: 'working',
      priority: 110,
      region: { kind: 'screen' },
      test: (text) => includesAny(text, ['esc to interrupt', 'ctrl+c to interrupt'])
    }
  ],
  pi: [
    {
      id: 'working-literal',
      state: 'working',
      priority: 100,
      region: { kind: 'screen' },
      test: (text) => includesAll(text, ['working...'])
    }
  ]
}

/** Whether this agent has any rules at all. */
export function isStateAwareAgent(agentId: string | undefined | null): boolean {
  return agentId != null && agentId in AGENT_RULES
}

/** The bottom `count` non-empty lines, joined back into a block. */
export function bottomNonEmptyLines(text: string, count: number): string {
  const lines = text.split('\n')
  const kept: string[] = []
  for (let i = lines.length - 1; i >= 0 && kept.length < count; i--) {
    if (lines[i].trim().length > 0) kept.unshift(lines[i])
  }
  return kept.join('\n')
}

function regionText(evidence: AgentStateEvidence, region: AgentRuleRegion): string | null {
  switch (region.kind) {
    case 'oscTitle':
      return evidence.oscTitle
    case 'screen':
      return evidence.screenTail
    case 'screenBottom':
      return evidence.screenTail == null
        ? null
        : bottomNonEmptyLines(evidence.screenTail, region.lines)
  }
}

export interface AgentStateResult {
  state: AgentTerminalState
  /** Id of the rule that decided it, or `null` when nothing matched. */
  matchedRule: string | null
}

/**
 * Picks the highest-priority matching rule.
 *
 * When nothing matches, the answer depends on what was available. With a screen
 * in hand, "no blocking prompt and no working hint" genuinely means idle. With
 * only a title — or nothing at all — it means we could not tell, and reporting
 * idle there would turn missing evidence into a confident wrong answer. The one
 * exception is an agent whose title alone settles it, which is expressed as a
 * real `idle` rule above rather than as a fallback.
 */
export function deriveAgentTerminalState(evidence: AgentStateEvidence): AgentStateResult {
  const rules = AGENT_RULES[evidence.agentId]
  if (!rules) return { state: 'unknown', matchedRule: null }

  let best: AgentRule | null = null
  for (const rule of rules) {
    if (best != null && best.priority >= rule.priority) continue
    const text = regionText(evidence, rule.region)
    if (text == null || !rule.test(text)) continue
    best = rule
  }

  if (best != null) return { state: best.state, matchedRule: best.id }
  return {
    state: evidence.screenTail == null ? 'unknown' : 'idle',
    matchedRule: null
  }
}
