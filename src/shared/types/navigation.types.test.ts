import { describe, expect, it } from 'vitest'
import {
  acceptRailRevision,
  applyRailReorder,
  createRailRevisionFence,
  defaultRailLayout,
  insertRailDivider,
  normalizeRailLayout,
  type RailLayoutEntry,
  railItemMobility,
  removeRailDivider,
  sameRailEntries
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
      'quickTerminals',
      'tools',
      'skills',
      'aiChannels',
      'mcp',
      'plugins',
      'scheduledTasks',
      'plugin-views'
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
      'item:quickTerminals',
      'divider:tools',
      'item:skills',
      'item:aiChannels',
      'item:mcp',
      'item:plugins',
      'item:scheduledTasks',
      'item:shortcuts',
      'divider:plugin-views'
    ])
    expect(normalized.entries.filter((entry) => entry.id === 'themes')).toHaveLength(0)
  })

  it('keeps user separators across a reload and removes only those', () => {
    const base = defaultRailLayout().entries
    const withSeparators = insertRailDivider(insertRailDivider(base, 1), 0)
    expect(withSeparators.slice(0, 3).map((entry) => `${entry.kind}:${entry.id}`)).toEqual([
      'divider:sep-2',
      'item:projects',
      'divider:sep-1'
    ])

    const reloaded = normalizeRailLayout({ schemaVersion: 1, revision: 3, entries: withSeparators })
    expect(reloaded.entries).toEqual(withSeparators)

    expect(removeRailDivider(withSeparators, 'sep-1').map((entry) => entry.id)).not.toContain(
      'sep-1'
    )
    expect(removeRailDivider(base, 'tools')).toEqual(base)
  })

  it('drops malformed and duplicate user separators on load', () => {
    const normalized = normalizeRailLayout({
      schemaVersion: 1,
      revision: 1,
      entries: [
        { kind: 'divider', id: 'sep-0' },
        { kind: 'divider', id: 'sep-x' },
        { kind: 'divider', id: 'sep-1' },
        { kind: 'divider', id: 'sep-1' }
      ]
    })
    expect(normalized.entries.filter((entry) => entry.id.startsWith('sep-'))).toEqual([
      { kind: 'divider', id: 'sep-1' }
    ])
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

  it('reconciles a missing plugins item immediately after a user-moved mcp', () => {
    const entries = defaultRailLayout().entries.filter((entry) => entry.id !== 'plugins')
    const mcp = entries.splice(
      entries.findIndex((entry) => entry.id === 'mcp'),
      1
    )[0]
    entries.unshift(mcp)
    const normalized = normalizeRailLayout({ schemaVersion: 1, revision: 7, entries })
    expect(normalized.entries.slice(0, 2).map((entry) => entry.id)).toEqual(['mcp', 'plugins'])
    expect(normalized.schemaVersion).toBe(1)
    expect(railItemMobility('plugins')).toBe('sortable')
  })

  it('preserves true collapse flags and normalizes false to absence on every divider kind', () => {
    const normalized = normalizeRailLayout({
      schemaVersion: 1,
      revision: 3,
      entries: [
        { kind: 'divider', id: 'workspace-contexts', collapsed: true },
        { kind: 'divider', id: 'tools', collapsed: false },
        { kind: 'divider', id: 'sep-1', collapsed: true },
        { kind: 'divider', id: 'plugin-views', collapsed: true }
      ]
    })
    expect(normalized.entries.find((entry) => entry.id === 'tools')).toEqual({
      kind: 'divider',
      id: 'tools'
    })
    for (const id of ['workspace-contexts', 'sep-1', 'plugin-views']) {
      expect(normalized.entries.find((entry) => entry.id === id)).toEqual({
        kind: 'divider',
        id,
        collapsed: true
      })
    }
    expect(normalizeRailLayout(normalized)).toEqual(normalized)
    expect(
      sameRailEntries(normalized.entries, insertRailDivider(normalized.entries, 0).slice(1))
    ).toBe(true)
    expect(
      sameRailEntries(
        [{ kind: 'divider', id: 'tools' }],
        [{ kind: 'divider', id: 'tools', collapsed: true }]
      )
    ).toBe(false)
    expect(
      sameRailEntries(removeRailDivider(normalized.entries, 'sep-9'), normalized.entries)
    ).toBe(true)
  })

  it.each([
    { kind: 'divider', id: 'tools', collapsed: 'true' },
    { kind: 'divider', id: 'tools', collapsed: null },
    { kind: 'item', id: 'mcp', collapsed: true },
    { kind: 'divider', id: 'tools', extra: true }
  ])('rejects malformed or misplaced collapse metadata: %j', (entry) => {
    expect(() => normalizeRailLayout({ schemaVersion: 1, revision: 1, entries: [entry] })).toThrow(
      'navigation layout is invalid'
    )
  })

  it.each([
    'by',
    'to-index'
  ] as const)('expands the owning divider when moving into its collapsed group via %s', (kind) => {
    const entries: RailLayoutEntry[] = [
      { kind: 'item', id: 'projects' },
      { kind: 'divider', id: 'tools', collapsed: true },
      { kind: 'item', id: 'mcp' },
      { kind: 'divider', id: 'plugin-views', collapsed: true }
    ]
    const moved = applyRailReorder(
      entries,
      'projects',
      kind === 'by' ? { kind, delta: 1 } : { kind, index: 1 }
    )
    expect(moved.changed).toBe(true)
    expect(moved.entries[0]).toEqual({ kind: 'divider', id: 'tools' })
    expect(moved.entries.at(-1)).toEqual({ kind: 'divider', id: 'plugin-views', collapsed: true })
    expect(entries[1]).toEqual({ kind: 'divider', id: 'tools', collapsed: true })
  })

  it('keeps collapse when moving within a group and when cloning unchanged entries', () => {
    const entries: RailLayoutEntry[] = [
      { kind: 'divider', id: 'tools', collapsed: true },
      { kind: 'item', id: 'mcp' },
      { kind: 'item', id: 'plugins' },
      { kind: 'divider', id: 'plugin-views', collapsed: true }
    ]
    expect(applyRailReorder(entries, 'mcp', { kind: 'by', delta: 1 }).entries[0]).toEqual(
      entries[0]
    )
    expect(applyRailReorder(entries, 'plugins', { kind: 'by', delta: 1 }).entries).toEqual(entries)
    expect(applyRailReorder(entries, 'shortcuts', { kind: 'by', delta: -1 }).entries[0]).toEqual({
      kind: 'divider',
      id: 'tools'
    })
  })

  it('keeps the render-only plugin header last through normalize, reorder, insert and remove', () => {
    const base = defaultRailLayout()
    const normalized = normalizeRailLayout({
      ...base,
      entries: [base.entries.at(-1), ...base.entries.slice(0, -1)]
    })
    expect(normalized).toEqual(base)
    const moved = applyRailReorder(base.entries, 'projects', { kind: 'to-index', index: 999 })
    expect(moved.entries.at(-2)).toEqual({ kind: 'item', id: 'projects' })
    expect(moved.entries.at(-1)).toEqual({ kind: 'divider', id: 'plugin-views' })
    expect(insertRailDivider(base.entries, 999).at(-1)).toEqual(base.entries.at(-1))
    expect(removeRailDivider(base.entries, 'plugin-views')).toEqual(base.entries)
    expect(applyRailReorder(base.entries, 'scheduledTasks', { kind: 'by', delta: 1 }).changed).toBe(
      false
    )
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
