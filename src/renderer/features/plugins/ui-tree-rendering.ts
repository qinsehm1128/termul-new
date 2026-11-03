import type {
  ActionEvent,
  UiAlign,
  UiGap,
  UiNode,
  UiTone,
  UiTree
} from '@shared/types/plugin.types'

/** The panel adds snapshot identity, event UUID and revision before crossing IPC. */
export type UiAction = Pick<ActionEvent, 'nodeId' | 'actionId' | 'kind' | 'value'>

export const UI_GAP_CLASSES: Record<UiGap, string> = {
  none: 'gap-0',
  xs: 'gap-1',
  sm: 'gap-2',
  md: 'gap-3',
  lg: 'gap-4'
}

export const UI_ALIGN_CLASSES: Record<UiAlign, string> = {
  start: 'items-start',
  center: 'items-center',
  end: 'items-end',
  stretch: 'items-stretch'
}

export const UI_TONE_CLASSES: Record<UiTone, string> = {
  default: 'text-foreground',
  muted: 'text-muted-foreground',
  danger: 'text-destructive'
}

/** Classify an action against an already validated tree; only text inputs need debounce. */
export function isInputAction(tree: UiTree, action: UiAction): boolean {
  if (action.kind !== 'change' || typeof action.value !== 'string') return false
  function matches(node: UiNode): boolean {
    if (node.type === 'input') {
      return node.id === action.nodeId && node.props.actionId === action.actionId
    }
    return (node.type === 'column' || node.type === 'row') && node.children.some(matches)
  }
  return matches(tree.root)
}
