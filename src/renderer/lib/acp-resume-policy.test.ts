import { describe, expect, it } from 'vitest'
import type { AgentCapabilities } from '@/lib/acp-api'
import { decideResume } from './acp-resume-policy'

describe('decideResume', () => {
  it('returns local when not connected', () => {
    expect(
      decideResume({
        connected: false,
        capabilities: { loadSession: true },
        localHistoryAvailable: true
      })
    ).toBe('local')
  })
  it('returns local when no capabilities', () => {
    expect(decideResume({ connected: true, capabilities: null, localHistoryAvailable: true })).toBe(
      'local'
    )
  })
  it('prefers resume for a Termul conversation when both capabilities are advertised', () => {
    const caps: AgentCapabilities = { loadSession: true, sessionCapabilities: { resume: {} } }
    expect(decideResume({ connected: true, capabilities: caps, localHistoryAvailable: true })).toBe(
      'resume'
    )
  })
  it('prefers load for a discovered session whose transcript is not local', () => {
    const caps: AgentCapabilities = { loadSession: true, sessionCapabilities: { resume: {} } }
    expect(
      decideResume({ connected: true, capabilities: caps, localHistoryAvailable: false })
    ).toBe('load')
  })
  it('uses resume when only resume is advertised', () => {
    const caps: AgentCapabilities = { loadSession: false, sessionCapabilities: { resume: {} } }
    expect(decideResume({ connected: true, capabilities: caps, localHistoryAvailable: true })).toBe(
      'resume'
    )
  })
  it('falls back to load when resume is unavailable', () => {
    const caps: AgentCapabilities = { loadSession: true, sessionCapabilities: {} }
    expect(decideResume({ connected: true, capabilities: caps, localHistoryAvailable: true })).toBe(
      'load'
    )
  })
  it('falls back to local when neither capability is present', () => {
    const caps: AgentCapabilities = { loadSession: false, sessionCapabilities: {} }
    expect(decideResume({ connected: true, capabilities: caps, localHistoryAvailable: true })).toBe(
      'local'
    )
  })
})
