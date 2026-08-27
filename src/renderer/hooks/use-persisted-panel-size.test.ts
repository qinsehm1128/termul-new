import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import {
  clampPanelSize,
  readPersistedPanelSize,
  usePersistedPanelSize
} from './use-persisted-panel-size'

describe('usePersistedPanelSize', () => {
  beforeEach(() => {
    window.localStorage.clear()
  })

  it('clamps and persists the size', () => {
    expect(clampPanelSize(10, 20, 80)).toBe(20)
    expect(readPersistedPanelSize('missing', 240, 180, 480)).toBe(240)
    window.localStorage.setItem('rail', '999')
    expect(readPersistedPanelSize('rail', 240, 180, 480)).toBe(480)

    const { result } = renderHook(() =>
      usePersistedPanelSize('rail', { initial: 240, min: 180, max: 480 })
    )
    expect(result.current[0]).toBe(480)
    act(() => {
      result.current[1](200)
    })
    expect(result.current[0]).toBe(200)
    expect(window.localStorage.getItem('rail')).toBe('200')
  })
})
