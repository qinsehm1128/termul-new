import { defaultRailLayout, type RailDividerId } from '@shared/types/navigation.types'
import { beforeEach, describe, expect, it } from 'vitest'
import { resetNavigationStore, useNavigationStore } from './navigation-store'

const state = () => useNavigationStore.getState()
const divider = (id: RailDividerId) =>
  state().layout.entries.find((entry) => entry.kind === 'divider' && entry.id === id)

describe('navigation-store', () => {
  beforeEach(() => resetNavigationStore())

  it.each([
    'workspace-contexts',
    'tools',
    'plugin-views'
  ] as const)('toggles %s with a revision and canonical absent expanded state', (id) => {
    const original = state().layout
    state().toggleDivider(id)
    expect(divider(id)).toEqual({ kind: 'divider', id, collapsed: true })
    expect(state().layout.revision).toBe(original.revision + 1)
    expect(original.entries.find((entry) => entry.id === id)).toEqual({ kind: 'divider', id })
    state().toggleDivider(id)
    expect(divider(id)).toEqual({ kind: 'divider', id })
    expect(state().layout.revision).toBe(original.revision + 2)
  })

  it('toggles a user divider and ignores a missing divider without revising', () => {
    const original = state().layout
    state().toggleDivider('sep-9')
    expect(state().layout).toBe(original)
    state().insertDivider(1)
    state().toggleDivider('sep-1')
    expect(divider('sep-1')).toEqual({ kind: 'divider', id: 'sep-1', collapsed: true })
    state().removeDivider('sep-1')
    expect(divider('sep-1')).toBeUndefined()
  })

  it('roundtrips collapse flags through hydrate and resumes revision fencing', () => {
    state().toggleDivider('tools')
    state().toggleDivider('plugin-views')
    const saved = JSON.parse(JSON.stringify(state().layout))
    resetNavigationStore({ isLoaded: false })
    expect(state().isLoaded).toBe(false)
    expect(state().hydrateLayout(saved)).toEqual(saved)
    expect(state().isLoaded).toBe(true)
    state().toggleDivider('tools')
    expect(state().layout.revision).toBe(saved.revision + 1)
    expect(divider('tools')).toEqual({ kind: 'divider', id: 'tools' })
    expect(divider('plugin-views')).toEqual({
      kind: 'divider',
      id: 'plugin-views',
      collapsed: true
    })
  })

  it('hydrates legacy layouts with plugins after the moved mcp and sidebar header last', () => {
    const entries = defaultRailLayout().entries.filter(
      (entry) => !['plugins', 'plugin-views', 'mcp'].includes(entry.id)
    )
    entries.unshift({ kind: 'item', id: 'mcp' })
    state().hydrateLayout({ schemaVersion: 1, revision: 12, entries })
    expect(
      state()
        .layout.entries.slice(0, 2)
        .map((entry) => entry.id)
    ).toEqual(['mcp', 'plugins'])
    expect(state().layout.entries.at(-1)).toEqual({ kind: 'divider', id: 'plugin-views' })
    state().toggleDivider('plugin-views')
    expect(state().layout.revision).toBe(13)
  })

  it.each([
    'keyboard',
    'indexed'
  ] as const)('expands a newly entered collapsed group with %s movement in one revision', (method) => {
    state().toggleDivider('tools')
    state().toggleDivider('plugin-views')
    const before = state().layout.revision
    if (method === 'keyboard') state().moveItemBy('quickTerminals', 1)
    else
      state().moveItemToIndex(
        'projects',
        state().layout.entries.findIndex((entry) => entry.id === 'tools')
      )
    expect(divider('tools')).toEqual({ kind: 'divider', id: 'tools' })
    expect(divider('plugin-views')).toEqual({
      kind: 'divider',
      id: 'plugin-views',
      collapsed: true
    })
    expect(state().layout.revision).toBe(before + 1)
  })

  it('preserves collapse across unrelated edits and cannot move items into the sidebar-only group', () => {
    state().toggleDivider('workspace-contexts')
    state().toggleDivider('plugin-views')
    state().moveItemBy('mcp', -1)
    state().insertDivider(999)
    state().pinUtility('plugins')
    state().removeDivider('plugin-views')
    state().moveItemToIndex('plugins', 999)
    expect(divider('workspace-contexts')).toEqual({
      kind: 'divider',
      id: 'workspace-contexts',
      collapsed: true
    })
    expect(state().layout.entries.at(-2)).toEqual({ kind: 'item', id: 'plugins' })
    expect(state().layout.entries.at(-1)).toEqual({
      kind: 'divider',
      id: 'plugin-views',
      collapsed: true
    })
    const before = state().layout
    state().moveItemBy('plugins', 1)
    expect(state().layout).toBe(before)
  })
})
