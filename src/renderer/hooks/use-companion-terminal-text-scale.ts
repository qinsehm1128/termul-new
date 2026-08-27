import { useEffect, useState } from 'react'
import {
  getCompanionTerminalTextScale,
  nudgeCompanionTerminalTextScale,
  setCompanionTerminalTextScale,
  subscribeCompanionTerminalTextScale
} from '@/lib/companion-terminal-text-scale'

export function useCompanionTerminalTextScale(): {
  scale: number
  percent: number
  setScale: (value: number, snap?: boolean) => number
  nudge: (direction: 1 | -1) => number
} {
  const [scale, setScaleState] = useState(getCompanionTerminalTextScale)

  useEffect(() => subscribeCompanionTerminalTextScale(setScaleState), [])

  return {
    scale,
    percent: Math.round(scale * 100),
    setScale: setCompanionTerminalTextScale,
    nudge: (direction) =>
      setCompanionTerminalTextScale(nudgeCompanionTerminalTextScale(scale, direction))
  }
}
