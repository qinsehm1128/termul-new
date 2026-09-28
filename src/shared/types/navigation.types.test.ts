import { describe, expect, it } from 'vitest'
import {
  acceptRailRevision,
  createRailRevisionFence,
  defaultRailLayout,
  normalizeRailLayout,
  railItemMobility
} from './navigation.types'

describe('activity rail layout', () => {
  it('uses the current rail order when no layout has been saved', () => {
    expect(normalizeRailLayout(undefined)).toEqual(defaultRailLayout())
    expect(defaultRailLayout().entries.map((entry) => entry.id)).toEqual([
      'projects',
      'terminals',
      'gitChanges',
      'gitHistory',
      'ssh',
      'workspace-contexts',
      'conversations',
      'tools',
      'skills',
      'aiChannels',
      'mcp',
      'scheduledTasks'
    ])
    expect(railItemMobility('preferences')).toBe('utility')
    expect(railItemMobility('projects')).toBe('sortable')
  })

  it('drops unknown and duplicate entries and restores missing defaults in place', () => {
    const normalized = normalizeRailLayout({
      schemaVersion: 1,
      revision: 4,
      entries: [
        { kind: 'item', id: 'projects' },
        { kind: 'item', id: 'projects' },
        { kind: 'item', id: 'brand' },
        { kind: 'item', id: 'not-a-rail-item' },
        { kind: 'divider', id: 'made-up' },
        { kind: 'item', id: 'mcp' },
        { kind: 'item', id: 'shortcuts' }
      ]
    })

    expect(normalized.revision).toBe(4)
    expect(normalized.entries.map((entry) => `${entry.kind}:${entry.id}`)).toEqual([
      'item:projects',
      'item:terminals',
      'item:gitChanges',
      'item:gitHistory',
      'item:ssh',
      'divider:workspace-contexts',
      'item:conversations',
      'divider:tools',
      'item:skills',
      'item:aiChannels',
      'item:mcp',
      'item:scheduledTasks',
      'item:shortcuts'
    ])
    expect(normalized.entries.filter((entry) => entry.id === 'themes')).toHaveLength(0)
  })

  it('preserves a user-moved item while inserting a missing neighbor after its predecessor', () => {
    const normalized = normalizeRailLayout({
      schemaVersion: 1,
      revision: 2,
      entries: [
        { kind: 'item', id: 'mcp' },
        { kind: 'item', id: 'projects' }
      ]
    })
    const ids = normalized.entries.map((entry) => entry.id)
    expect(ids.indexOf('mcp')).toBeLessThan(ids.indexOf('projects'))
    expect(ids[ids.indexOf('projects') + 1]).toBe('terminals')
  })

  it('fails closed on a bad document or a credential field, and fences revisions', () => {
    expect(() => normalizeRailLayout({ schemaVersion: 2, revision: 1, entries: [] })).toThrow(
      'navigation layout is invalid'
    )
    expect(() =>
      normalizeRailLayout({
        schemaVersion: 1,
        revision: 1,
        entries: [],
        apiKey: 'sk-canary-secret'
      })
    ).toThrow('navigation layout contains a forbidden credential field')

    const fence = createRailRevisionFence()
    expect(() => acceptRailRevision(fence, 0)).toThrow('navigation revision is invalid')
    acceptRailRevision(fence, 1)
    expect(() => acceptRailRevision(fence, 1)).toThrow('navigation revision is stale')
    acceptRailRevision(fence, 2)
    expect(fence.lastAccepted).toBe(2)
  })
})
