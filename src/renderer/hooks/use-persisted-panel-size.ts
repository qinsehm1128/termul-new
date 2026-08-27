import { useCallback, useEffect, useState } from 'react'

export function clampPanelSize(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value))
}

export function readPersistedPanelSize(
  key: string,
  fallback: number,
  min: number,
  max: number
): number {
  try {
    const saved = window.localStorage?.getItem(key)
    if (!saved) return clampPanelSize(fallback, min, max)
    const parsed = Number.parseInt(saved, 10)
    if (Number.isNaN(parsed)) return clampPanelSize(fallback, min, max)
    return clampPanelSize(parsed, min, max)
  } catch {
    return clampPanelSize(fallback, min, max)
  }
}

export function usePersistedPanelSize(
  key: string,
  options: { initial: number; min: number; max: number }
): [number, (next: number) => void] {
  const { initial, min, max } = options
  const [size, setSize] = useState(() => readPersistedPanelSize(key, initial, min, max))

  const update = useCallback(
    (next: number) => {
      setSize(clampPanelSize(next, min, max))
    },
    [max, min]
  )

  useEffect(() => {
    try {
      window.localStorage?.setItem(key, String(size))
    } catch {
      // Ignore storage errors in restricted environments.
    }
  }, [key, size])

  return [size, update]
}
