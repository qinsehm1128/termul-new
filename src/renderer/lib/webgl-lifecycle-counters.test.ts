import { beforeEach, describe, expect, it } from 'vitest'
import {
  _resetWebglLifecycleCountersForTesting,
  getWebglLifecycleSnapshot,
  noteWebglAddonCreated,
  noteWebglAddonDisposed,
  noteWebglContextLost
} from './webgl-lifecycle-counters'

describe('webgl lifecycle counters', () => {
  beforeEach(() => {
    _resetWebglLifecycleCountersForTesting()
  })

  it('reports create/dispose totals and a never-negative live count', () => {
    noteWebglAddonCreated()
    noteWebglAddonCreated()
    expect(getWebglLifecycleSnapshot()).toEqual({
      created: 2,
      disposed: 0,
      live: 2,
      contextsLost: 0
    })

    noteWebglAddonDisposed()
    noteWebglContextLost()
    expect(getWebglLifecycleSnapshot()).toEqual({
      created: 2,
      disposed: 1,
      live: 1,
      contextsLost: 1
    })

    // An extra dispose (double-dispose path) must not report a negative live.
    noteWebglAddonDisposed()
    noteWebglAddonDisposed()
    expect(getWebglLifecycleSnapshot().live).toBe(0)
  })
})
