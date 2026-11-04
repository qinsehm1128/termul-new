import { defaultRailLayout } from '@shared/types/navigation.types'
import { describe, expect, it } from 'vitest'
import { type RailRowBox, railPointerDrop, railRestIndex } from './rail-drop'

// Collapsed contents have no DOM rows. Divider indexes still refer to the full
// persisted list, so a pointer can insert into the hidden group and expand it.
describe('rail drop targets with collapsed groups', () => {
  const rows: RailRowBox[] = [
    { kind: 'item', id: 'projects', entryIndex: 0, top: 0, height: 36 },
    { kind: 'divider', id: 'workspace-contexts', entryIndex: 5, top: 36, height: 24 },
    { kind: 'divider', id: 'tools', entryIndex: 8, top: 60, height: 24 },
    { kind: 'item', id: 'skills', entryIndex: 9, top: 84, height: 36 },
    { kind: 'divider', id: 'plugin-views', entryIndex: -1, top: 120, height: 24 }
  ]

  it('targets the start of a collapsed group below its divider', () => {
    expect(railPointerDrop(54, rows, 'skills')).toEqual({ beforeEntryIndex: 8, pin: false })
    const target = railPointerDrop(54, rows, 'projects')
    const entries = defaultRailLayout().entries
    expect(railRestIndex(entries, 'projects', target.beforeEntryIndex)).toBe(7)
  })

  it('ignores the render-only plugin header as an insertion boundary', () => {
    expect(railPointerDrop(140, rows, 'projects')).toEqual({ beforeEntryIndex: 10, pin: false })
  })

  it('retains utility pin targets below collapsed rows', () => {
    const withUtility: RailRowBox[] = [
      ...rows,
      { kind: 'utility', id: 'preferences', entryIndex: -1, top: 200, height: 36 }
    ]
    expect(railPointerDrop(220, withUtility, 'preferences').pin).toBe(true)
    expect(railPointerDrop(220, withUtility, 'projects').pin).toBe(false)
  })
})
