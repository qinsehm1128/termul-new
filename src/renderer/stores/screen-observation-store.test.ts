import type { ScreenObservationStatus } from '@shared/types/terminal-screen.types'
import {
  isScreenObservationReady,
  legacyScreenObservation,
  normalizeScreenObservation,
  usableHostScreenText
} from '@shared/types/terminal-screen.types'
import { beforeEach, describe, expect, it } from 'vitest'
import { useScreenObservationStore } from './screen-observation-store'

const ready: ScreenObservationStatus = {
  terminalScreenEngine: 'ghostty-vt',
  state: 'ready',
  reason: null,
  capturedSeq: 5,
  appliedSeq: 5,
  observerBuildId: 'build'
}

describe('host-issued screen observation cache and evidence contract', () => {
  beforeEach(() => useScreenObservationStore.setState({ byPtyId: {} }))

  it('uses unsupported/legacy-host for an absent older-host observation', () => {
    expect(normalizeScreenObservation(undefined)).toEqual(legacyScreenObservation())
    expect(isScreenObservationReady(legacyScreenObservation())).toBe(false)
  })

  it.each([
    { ...ready, state: 'future' },
    { ...ready, reason: 'future' },
    { ...ready, capturedSeq: -1 },
    { ...ready, appliedSeq: Number.MAX_SAFE_INTEGER + 1 },
    { ...ready, observerBuildId: 42 },
    { state: 'ready' },
    'ready'
  ])('unknown/malformed statuses fail closed: %j', (value) => {
    const status = normalizeScreenObservation(value)
    expect(status.state).toBe('unsupported')
    expect(status.reason).toBe('protocol')
    expect(isScreenObservationReady(status)).toBe(false)
  })

  it.each([
    { ...ready, capturedSeq: 6 },
    { ...ready, terminalScreenEngine: 'existing' as const },
    { ...ready, reason: 'timeout' as const }
  ])('never treats an inconsistent ready label as evidence: %j', (status) => {
    expect(normalizeScreenObservation(status).state).toBe('degraded')
    expect(
      usableHostScreenText({
        status,
        text: 'private screen',
        appliedSeq: status.appliedSeq,
        truncated: false
      })
    ).toBeNull()
  })

  it('allows host text only at the exact ready barrier', () => {
    expect(
      usableHostScreenText({ status: ready, text: 'screen', appliedSeq: 5, truncated: true })
    ).toBe('screen')
    expect(
      usableHostScreenText({ status: ready, text: 'screen', appliedSeq: 4, truncated: false })
    ).toBeNull()
    expect(
      usableHostScreenText({
        status: { ...ready, state: 'catching-up' },
        text: 'screen',
        appliedSeq: 5,
        truncated: false
      })
    ).toBeNull()
  })

  it('records actual mode independently of project settings and accepts summary envelope fields', () => {
    useScreenObservationStore.getState().recordHost('pty', {
      terminalScreenEngine: 'existing',
      screenObservation: { ...ready, terminalScreenEngine: 'existing', state: 'disabled' }
    })
    expect(useScreenObservationStore.getState().byPtyId.pty.terminalScreenEngine).toBe('existing')
    expect(useScreenObservationStore.getState().byPtyId.pty.status?.state).toBe('disabled')
  })

  it('a stale ready snapshot cannot recover degradation, but a new claim generation resets local cache', () => {
    const store = useScreenObservationStore.getState()
    store.recordHost('pty', {
      terminalScreenEngine: 'ghostty-vt',
      claim: 'one',
      status: { ...ready, state: 'degraded', reason: 'overflow' }
    })
    store.recordHost('pty', { status: ready })
    expect(useScreenObservationStore.getState().byPtyId.pty.status?.state).toBe('degraded')
    store.recordHost('pty', { claim: 'two', status: ready })
    expect(useScreenObservationStore.getState().byPtyId.pty.status?.state).toBe('ready')
    store.clear('pty')
    expect(useScreenObservationStore.getState().byPtyId.pty).toBeUndefined()
  })
})
