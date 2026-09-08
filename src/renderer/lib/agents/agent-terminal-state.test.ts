import { describe, expect, it } from 'vitest'
import {
  type AgentStateEvidence,
  bottomNonEmptyLines,
  deriveAgentTerminalState,
  isStateAwareAgent
} from './agent-terminal-state'

function evidence(partial: Partial<AgentStateEvidence> & { agentId: string }): AgentStateEvidence {
  return { oscTitle: null, screenTail: null, ...partial }
}

describe('deriveAgentTerminalState', () => {
  describe('missing evidence never reads as idle', () => {
    it('reports unknown when there is neither a title nor a screen', () => {
      const result = deriveAgentTerminalState(evidence({ agentId: 'claude-code' }))
      expect(result.state).toBe('unknown')
      expect(result.matchedRule).toBeNull()
    })

    it('reports unknown for an agent with no rules, even with full evidence', () => {
      const result = deriveAgentTerminalState(
        evidence({ agentId: 'some-future-agent', oscTitle: 'busy', screenTail: 'working...' })
      )
      expect(result.state).toBe('unknown')
    })

    it('reports unknown when a title is present but settles nothing', () => {
      // claude's title carries a marker when it means something. A bare project
      // name cannot rule out a permission prompt sitting on the screen.
      const result = deriveAgentTerminalState(
        evidence({ agentId: 'claude-code', oscTitle: 'my-project' })
      )
      expect(result.state).toBe('unknown')
    })

    it('falls back to idle only once a screen has been seen', () => {
      const result = deriveAgentTerminalState(
        evidence({ agentId: 'claude-code', screenTail: 'nothing interesting here' })
      )
      expect(result.state).toBe('idle')
      expect(result.matchedRule).toBeNull()
    })

    it('treats an empty screen as evidence, unlike a missing one', () => {
      expect(deriveAgentTerminalState(evidence({ agentId: 'pi', screenTail: '' })).state).toBe(
        'idle'
      )
      expect(deriveAgentTerminalState(evidence({ agentId: 'pi' })).state).toBe('unknown')
    })
  })

  describe('claude-code', () => {
    it('reads the braille spinner in the title as working', () => {
      const result = deriveAgentTerminalState(
        evidence({ agentId: 'claude-code', oscTitle: '⠋ my-project' })
      )
      expect(result).toEqual({ state: 'working', matchedRule: 'title-busy' })
    })

    it('reads the half-circle spinner in the title as working', () => {
      expect(
        deriveAgentTerminalState(evidence({ agentId: 'claude-code', oscTitle: '◐ my-project' }))
          .state
      ).toBe('working')
    })

    it('reads the settled marker in the title as idle', () => {
      const result = deriveAgentTerminalState(
        evidence({ agentId: 'claude-code', oscTitle: '✳ my-project' })
      )
      expect(result).toEqual({ state: 'idle', matchedRule: 'title-settled' })
    })

    it('lets a visible permission prompt outrank a settled title', () => {
      // The title says nothing is running, but the screen is waiting on the
      // user. This is the case a title-only reading gets wrong.
      const result = deriveAgentTerminalState(
        evidence({
          agentId: 'claude-code',
          oscTitle: '✳ my-project',
          screenTail: [
            'Bash(rm -rf build)',
            'Do you want to proceed?',
            '❯ 1. Yes',
            '  2. No',
            'esc to cancel'
          ].join('\n')
        })
      )
      expect(result).toEqual({ state: 'blocked', matchedRule: 'permission-prompt' })
    })

    it('lets a spinning title outrank an interrupt hint left on screen', () => {
      const result = deriveAgentTerminalState(
        evidence({
          agentId: 'claude-code',
          oscTitle: '⠙ my-project',
          screenTail: '⏵ Thinking… (esc to interrupt)'
        })
      )
      expect(result.state).toBe('working')
      expect(result.matchedRule).toBe('title-busy')
    })

    it('requires both halves of the permission prompt, not just the cancel hint', () => {
      const result = deriveAgentTerminalState(
        evidence({ agentId: 'claude-code', screenTail: 'esc to cancel' })
      )
      expect(result.state).toBe('idle')
    })

    it('ignores a permission prompt scrolled out of the bottom lines', () => {
      const stale = ['Do you want to proceed?', 'esc to cancel']
        .concat(Array.from({ length: 20 }, (_, i) => `output line ${i}`))
        .join('\n')
      expect(
        deriveAgentTerminalState(evidence({ agentId: 'claude-code', screenTail: stale })).state
      ).toBe('idle')
    })
  })

  describe('codex', () => {
    it('reads Action Required in the title as blocked', () => {
      const result = deriveAgentTerminalState(
        evidence({ agentId: 'codex', oscTitle: 'Action Required — codex' })
      )
      expect(result).toEqual({ state: 'blocked', matchedRule: 'title-action-required' })
    })

    it('reads the spinner in the title as working', () => {
      expect(
        deriveAgentTerminalState(evidence({ agentId: 'codex', oscTitle: '⠹ codex' })).state
      ).toBe('working')
    })

    it('settles on idle from the title alone, with no screen', () => {
      // codex keeps its title current, so a marker-free title is real evidence.
      const result = deriveAgentTerminalState(evidence({ agentId: 'codex', oscTitle: 'codex' }))
      expect(result).toEqual({ state: 'idle', matchedRule: 'title-settled' })
    })

    it('does not settle on an empty title', () => {
      expect(deriveAgentTerminalState(evidence({ agentId: 'codex', oscTitle: '   ' })).state).toBe(
        'unknown'
      )
    })

    it('keeps blocked ahead of the settled reading', () => {
      expect(
        deriveAgentTerminalState(evidence({ agentId: 'codex', oscTitle: 'Action Required' })).state
      ).toBe('blocked')
    })
  })

  describe('screen-only agents', () => {
    it('reads a cursor write approval as blocked', () => {
      const result = deriveAgentTerminalState(
        evidence({
          agentId: 'cursor',
          screenTail: 'Write to this file?\nProceed (y)\nReject & propose changes'
        })
      )
      expect(result).toEqual({ state: 'blocked', matchedRule: 'write-file-approval' })
    })

    it('reads a cursor stop hint as working', () => {
      expect(
        deriveAgentTerminalState(
          evidence({ agentId: 'cursor', screenTail: 'Generating…\nctrl+c to stop' })
        ).state
      ).toBe('working')
    })

    it('reads a cursor spinner line as working', () => {
      expect(
        deriveAgentTerminalState(evidence({ agentId: 'cursor', screenTail: '⬢ Thinking' })).state
      ).toBe('working')
    })

    it('reads a gemini confirmation as blocked', () => {
      expect(
        deriveAgentTerminalState(
          evidence({ agentId: 'gemini-cli', screenTail: '│ Apply this change?\n│ Yes' })
        ).state
      ).toBe('blocked')
    })

    it('reads a gemini cancel hint as working', () => {
      expect(
        deriveAgentTerminalState(
          evidence({ agentId: 'gemini-cli', screenTail: 'Thinking (esc to cancel)' })
        ).state
      ).toBe('working')
    })

    it('reads an opencode permission prompt as blocked', () => {
      expect(
        deriveAgentTerminalState(
          evidence({ agentId: 'opencode', screenTail: '△ Permission required\nenter confirm' })
        ).state
      ).toBe('blocked')
    })

    it('keeps an opencode permission prompt ahead of its interrupt hint', () => {
      const result = deriveAgentTerminalState(
        evidence({
          agentId: 'opencode',
          screenTail: 'esc to interrupt\n△ Permission required'
        })
      )
      expect(result.state).toBe('blocked')
    })

    it('reads pi working', () => {
      expect(
        deriveAgentTerminalState(evidence({ agentId: 'pi', screenTail: 'Working...' })).state
      ).toBe('working')
    })

    it('reports pi idle once the literal is gone', () => {
      expect(
        deriveAgentTerminalState(evidence({ agentId: 'pi', screenTail: 'done\n$ ' })).state
      ).toBe('idle')
    })
  })

  it('matches case-insensitively, since these strings are prose', () => {
    expect(
      deriveAgentTerminalState(evidence({ agentId: 'opencode', screenTail: 'PERMISSION REQUIRED' }))
        .state
    ).toBe('blocked')
  })
})

describe('isStateAwareAgent', () => {
  it('accepts every agent in the terminal registry', () => {
    for (const id of ['claude-code', 'codex', 'cursor', 'gemini-cli', 'opencode', 'pi']) {
      expect(isStateAwareAgent(id), id).toBe(true)
    }
  })

  it('rejects unknown ids and nullish input', () => {
    expect(isStateAwareAgent('zsh')).toBe(false)
    expect(isStateAwareAgent(null)).toBe(false)
    expect(isStateAwareAgent(undefined)).toBe(false)
  })
})

describe('bottomNonEmptyLines', () => {
  it('keeps the last N non-empty lines in original order', () => {
    expect(bottomNonEmptyLines('a\n\nb\n\n\nc\nd', 3)).toBe('b\nc\nd')
  })

  it('returns everything when there are fewer lines than asked for', () => {
    expect(bottomNonEmptyLines('a\nb', 10)).toBe('a\nb')
  })

  it('returns an empty string for a blank block', () => {
    expect(bottomNonEmptyLines('\n\n   \n', 5)).toBe('')
  })
})
