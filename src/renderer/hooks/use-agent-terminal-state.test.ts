import { describe, expect, it } from 'vitest'
import type { Terminal } from '@/types/project'
import {
  isAgentStateCandidate,
  planTerminalScan,
  type ScanMemory
} from './use-agent-terminal-state'

function terminal(partial: Partial<Terminal> = {}): Terminal {
  return { id: 'term-1', name: 'agent', ...partial } as Terminal
}

describe('planTerminalScan', () => {
  it('scans a terminal it has never seen', () => {
    const { scan, next } = planTerminalScan({ lastActivityTimestamp: undefined }, undefined)
    // A terminal that has been idle since before the app started still needs one
    // read, or it reports unknown forever.
    expect(scan).toBe(true)
    expect(next.settleRemaining).toBeGreaterThan(0)
  })

  it('scans again when new output arrived', () => {
    const memory: ScanMemory = { activityAt: 100, settleRemaining: 0 }
    const { scan, next } = planTerminalScan({ lastActivityTimestamp: 200 }, memory)
    expect(scan).toBe(true)
    expect(next.activityAt).toBe(200)
  })

  it('rearms the settle passes on new output', () => {
    const memory: ScanMemory = { activityAt: 100, settleRemaining: 0 }
    const { next } = planTerminalScan({ lastActivityTimestamp: 200 }, memory)
    expect(next.settleRemaining).toBeGreaterThan(0)
  })

  it('keeps scanning for a bounded number of passes after output stops', () => {
    // xterm writes asynchronously, so the frame that says "idle now" can land
    // after the activity timestamp that triggered the scan.
    let memory = planTerminalScan({ lastActivityTimestamp: 100 }, undefined).next
    const quiet = { lastActivityTimestamp: 100 }

    let passes = 0
    for (;;) {
      const result = planTerminalScan(quiet, memory)
      memory = result.next
      if (!result.scan) break
      passes++
      expect(passes).toBeLessThan(10)
    }
    expect(passes).toBe(2)
  })

  it('stops scanning entirely once the settle passes are spent', () => {
    let memory: ScanMemory = { activityAt: 100, settleRemaining: 0 }
    const quiet = { lastActivityTimestamp: 100 }
    for (let i = 0; i < 5; i++) {
      const result = planTerminalScan(quiet, memory)
      expect(result.scan).toBe(false)
      memory = result.next
    }
  })

  it('treats a terminal that never reported activity as quiet after settling', () => {
    let memory = planTerminalScan({ lastActivityTimestamp: undefined }, undefined).next
    for (let i = 0; i < 2; i++) {
      memory = planTerminalScan({ lastActivityTimestamp: undefined }, memory).next
    }
    expect(planTerminalScan({ lastActivityTimestamp: undefined }, memory).scan).toBe(false)
  })
})

describe('isAgentStateCandidate', () => {
  it('accepts an agent terminal running a known agent', () => {
    expect(isAgentStateCandidate(terminal({ kind: 'agent', agentId: 'claude-code' }))).toBe(true)
  })

  it('rejects a plain shell', () => {
    expect(isAgentStateCandidate(terminal({ kind: 'shell' }))).toBe(false)
    expect(isAgentStateCandidate(terminal({ kind: undefined }))).toBe(false)
  })

  it('rejects an agent terminal with no rules for its agent', () => {
    expect(isAgentStateCandidate(terminal({ kind: 'agent', agentId: 'some-new-cli' }))).toBe(false)
  })

  it('rejects an agent terminal with no agent id', () => {
    expect(isAgentStateCandidate(terminal({ kind: 'agent', agentId: undefined }))).toBe(false)
  })
})
