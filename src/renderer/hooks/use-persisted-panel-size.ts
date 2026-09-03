import { acceptedBrandValues } from '@shared/brand'
import { useCallback, useEffect, useState } from 'react'

export function clampPanelSize(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value))
}

/**
 * `key` plus the same key under every older `localStorage` prefix.
 *
 * Panel sizes are written under one brand-prefixed key, so the moment that
 * prefix flips every persisted sidebar / explorer / rail width becomes
 * unreachable and the panels snap back to their defaults. Rewriting the
 * caller's key is enough — the bare suffix is brand-free.
 *
 * Returns just `key` while the prefixes are still equal, and for any key that
 * does not carry the current prefix at all (a caller that has not flipped yet,
 * which is every caller this wave).
 */
function readableKeys(key: string): string[] {
  const prefixes = acceptedBrandValues('storageKeyPrefix')
  const current = prefixes[0]
  if (prefixes.length === 1 || !key.startsWith(current)) return [key]
  const bare = key.slice(current.length)
  return [key, ...prefixes.slice(1).map((prefix) => `${prefix}${bare}`)]
}

export function readPersistedPanelSize(
  key: string,
  fallback: number,
  min: number,
  max: number
): number {
  try {
    for (const stored of readableKeys(key)) {
      const saved = window.localStorage?.getItem(stored)
      if (!saved) continue
      // First key that holds anything decides — including when what it holds
      // is garbage, which is the single-key behaviour this preserves.
      const parsed = Number.parseInt(saved, 10)
      if (Number.isNaN(parsed)) return clampPanelSize(fallback, min, max)
      return clampPanelSize(parsed, min, max)
    }
    return clampPanelSize(fallback, min, max)
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
