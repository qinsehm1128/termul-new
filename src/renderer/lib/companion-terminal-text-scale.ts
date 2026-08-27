export const COMPANION_TERMINAL_TEXT_SCALES = [0.5, 0.75, 1, 1.25, 1.5, 2] as const

export type CompanionTerminalTextScale = (typeof COMPANION_TERMINAL_TEXT_SCALES)[number]

export const DEFAULT_COMPANION_TERMINAL_TEXT_SCALE = 1.25

export const COMPANION_TERMINAL_TEXT_SCALE_STORAGE_KEY = 'termul.companion.terminalTextScale'

const MIN_SCALE = COMPANION_TERMINAL_TEXT_SCALES[0]
const MAX_SCALE = COMPANION_TERMINAL_TEXT_SCALES[COMPANION_TERMINAL_TEXT_SCALES.length - 1]

const listeners = new Set<(scale: number) => void>()
let currentScale = readStoredCompanionTerminalTextScale()

export function clampCompanionTerminalTextScale(value: number): number {
  if (!Number.isFinite(value)) {
    return DEFAULT_COMPANION_TERMINAL_TEXT_SCALE
  }
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, value))
}

export function snapCompanionTerminalTextScale(value: number): number {
  const clamped = clampCompanionTerminalTextScale(value)
  let best: CompanionTerminalTextScale = COMPANION_TERMINAL_TEXT_SCALES[0]
  let bestDelta = Math.abs(clamped - best)
  for (const preset of COMPANION_TERMINAL_TEXT_SCALES) {
    const delta = Math.abs(clamped - preset)
    if (delta < bestDelta) {
      best = preset
      bestDelta = delta
    }
  }
  return best
}

export function nudgeCompanionTerminalTextScale(value: number, direction: 1 | -1): number {
  const snapped = snapCompanionTerminalTextScale(value)
  const index = COMPANION_TERMINAL_TEXT_SCALES.indexOf(snapped as CompanionTerminalTextScale)
  const next = index + direction
  if (next < 0) return COMPANION_TERMINAL_TEXT_SCALES[0]
  if (next >= COMPANION_TERMINAL_TEXT_SCALES.length) {
    return COMPANION_TERMINAL_TEXT_SCALES[COMPANION_TERMINAL_TEXT_SCALES.length - 1]
  }
  return COMPANION_TERMINAL_TEXT_SCALES[next]
}

export function getCompanionTerminalTextScale(): number {
  return currentScale
}

export function setCompanionTerminalTextScale(value: number, snap = true): number {
  const next = snap ? snapCompanionTerminalTextScale(value) : clampCompanionTerminalTextScale(value)
  if (next === currentScale) {
    return currentScale
  }
  currentScale = next
  writeStoredCompanionTerminalTextScale(next)
  for (const listener of listeners) {
    listener(next)
  }
  return next
}

export function subscribeCompanionTerminalTextScale(listener: (scale: number) => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function readStoredCompanionTerminalTextScale(): number {
  if (typeof localStorage === 'undefined') {
    return DEFAULT_COMPANION_TERMINAL_TEXT_SCALE
  }
  try {
    const raw = localStorage.getItem(COMPANION_TERMINAL_TEXT_SCALE_STORAGE_KEY)
    if (raw == null) {
      return DEFAULT_COMPANION_TERMINAL_TEXT_SCALE
    }
    return snapCompanionTerminalTextScale(Number(raw))
  } catch {
    return DEFAULT_COMPANION_TERMINAL_TEXT_SCALE
  }
}

function writeStoredCompanionTerminalTextScale(value: number): void {
  if (typeof localStorage === 'undefined') {
    return
  }
  try {
    localStorage.setItem(COMPANION_TERMINAL_TEXT_SCALE_STORAGE_KEY, String(value))
  } catch {
    // Private mode can reject writes; in-memory scale still works.
  }
}
