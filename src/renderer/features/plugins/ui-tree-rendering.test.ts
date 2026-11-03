import type { UiNode, UiTree } from '@shared/types/plugin.types'
import { describe, expect, it } from 'vitest'
import {
  isInputAction,
  UI_ALIGN_CLASSES,
  UI_GAP_CLASSES,
  UI_TONE_CLASSES,
  type UiAction
} from './ui-tree-rendering'

const input: UiNode = {
  type: 'input',
  id: 'query',
  props: { label: 'Query', value: '', actionId: 'search' }
}
const tree: UiTree = {
  schemaVersion: 1,
  viewId: 'main',
  root: {
    type: 'column',
    id: 'root',
    props: {},
    children: [
      { type: 'row', id: 'nested', props: {}, children: [input] },
      {
        type: 'select',
        id: 'choice',
        props: {
          label: 'Choice',
          value: 'a',
          options: [{ value: 'a', label: 'A' }],
          actionId: 'choose'
        }
      },
      { type: 'button', id: 'save', props: { label: 'Save', actionId: 'save' } }
    ]
  }
}
const action: UiAction = { nodeId: 'query', actionId: 'search', kind: 'change', value: 'new' }

describe('isInputAction', () => {
  it('recognizes nested and root inputs for panel-owned debounce', () => {
    expect(isInputAction(tree, action)).toBe(true)
    expect(isInputAction({ ...tree, root: input }, action)).toBe(true)
  })

  it.each<UiAction>([
    { ...action, nodeId: 'missing' },
    { ...action, actionId: 'wrong' },
    { ...action, kind: 'click' },
    { ...action, value: false },
    { nodeId: 'choice', actionId: 'choose', kind: 'change', value: 'a' },
    { nodeId: 'save', actionId: 'save', kind: 'click', value: null }
  ])('does not debounce mismatched or non-input action %j', (candidate) => {
    expect(isInputAction(tree, candidate)).toBe(false)
  })
})

describe('host-owned theme maps', () => {
  it('provides every closed schema enum without arbitrary plugin class construction', () => {
    expect(UI_GAP_CLASSES).toEqual({
      none: 'gap-0',
      xs: 'gap-1',
      sm: 'gap-2',
      md: 'gap-3',
      lg: 'gap-4'
    })
    expect(UI_ALIGN_CLASSES).toEqual({
      start: 'items-start',
      center: 'items-center',
      end: 'items-end',
      stretch: 'items-stretch'
    })
    expect(UI_TONE_CLASSES).toEqual({
      default: 'text-foreground',
      muted: 'text-muted-foreground',
      danger: 'text-destructive'
    })
  })
})
