import { beforeEach, describe, expect, it } from 'vitest'
import {
  COMPANION_TERMINAL_TEXT_SCALE_STORAGE_KEY,
  clampCompanionTerminalTextScale,
  DEFAULT_COMPANION_TERMINAL_TEXT_SCALE,
  getCompanionTerminalTextScale,
  nudgeCompanionTerminalTextScale,
  setCompanionTerminalTextScale,
  snapCompanionTerminalTextScale
} from './companion-terminal-text-scale'

describe('companion terminal text scale', () => {
  beforeEach(() => {
    setCompanionTerminalTextScale(DEFAULT_COMPANION_TERMINAL_TEXT_SCALE)
  })

  it('clamps and snaps to operate-friendly presets', () => {
    expect(clampCompanionTerminalTextScale(0.1)).toBe(0.5)
    expect(clampCompanionTerminalTextScale(4)).toBe(2)
    expect(snapCompanionTerminalTextScale(1.2)).toBe(1.25)
    expect(snapCompanionTerminalTextScale(1.4)).toBe(1.5)
  })

  it('nudges one preset at a time', () => {
    expect(nudgeCompanionTerminalTextScale(1.25, 1)).toBe(1.5)
    expect(nudgeCompanionTerminalTextScale(1.25, -1)).toBe(1)
    expect(nudgeCompanionTerminalTextScale(0.5, -1)).toBe(0.5)
    expect(nudgeCompanionTerminalTextScale(2, 1)).toBe(2)
  })

  it('persists the snapped scale for the phone viewport', () => {
    expect(setCompanionTerminalTextScale(1.42)).toBe(1.5)
    expect(getCompanionTerminalTextScale()).toBe(1.5)
    if (typeof localStorage !== 'undefined') {
      expect(localStorage.getItem(COMPANION_TERMINAL_TEXT_SCALE_STORAGE_KEY)).toBe('1.5')
    }
  })
})
