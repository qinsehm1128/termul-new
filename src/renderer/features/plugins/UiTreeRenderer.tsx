import { parseUiTree, type UiNode, type UiTree } from '@shared/types/plugin.types'
import { useEffect, useId, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from '@/components/ui/select'
import { Separator } from '@/components/ui/separator'
import { Switch } from '@/components/ui/switch'
import { logFrontendError } from '@/lib/log-api'
import {
  UI_ALIGN_CLASSES,
  UI_GAP_CLASSES,
  UI_TONE_CLASSES,
  type UiAction
} from './ui-tree-rendering'

export interface UiTreeRendererProps {
  tree: unknown
  disabled?: boolean
  onAction: (action: UiAction) => void
}

interface NodeProps {
  node: UiNode
  tree: UiTree
  disabled: boolean
  onAction: (action: UiAction) => void
}

/** Keep newer edits when the server acknowledges an earlier emitted value. */
function useControlDraft(serverValue: string | boolean, tree: UiTree) {
  const [draft, setDraft] = useState({
    tree,
    serverValue,
    value: serverValue,
    pending: [] as (string | boolean)[]
  })
  if (draft.tree !== tree || draft.serverValue !== serverValue) {
    const acknowledged = serverValue === draft.serverValue ? -1 : draft.pending.indexOf(serverValue)
    const authoritativeReset = acknowledged < 0 && serverValue !== draft.serverValue
    const pending = authoritativeReset ? [] : draft.pending.slice(acknowledged + 1)
    setDraft({
      tree,
      serverValue,
      pending,
      value: pending.length ? pending[pending.length - 1] : serverValue
    })
  }
  const update = (value: string | boolean) => {
    setDraft((current) => ({ ...current, value, pending: [...current.pending, value] }))
  }
  return [draft.value, update] as const
}

type ControlNode = Extract<UiNode, { type: 'input' | 'toggle' | 'select' }>

function DraftControl({
  node,
  tree,
  disabled,
  onAction,
  id
}: NodeProps & { node: ControlNode; id: string }) {
  const [value, update] = useControlDraft(
    node.type === 'toggle' ? node.props.checked : node.props.value,
    tree
  )
  const blocked = disabled || node.props.disabled
  const change = (next: string | boolean) => {
    if (blocked) return
    update(next)
    onAction({ nodeId: node.id, actionId: node.props.actionId, kind: 'change', value: next })
  }
  if (node.type === 'toggle') {
    return (
      <div className="flex items-center gap-2">
        <Switch id={id} checked={Boolean(value)} disabled={blocked} onCheckedChange={change} />
        <label htmlFor={id} className="text-sm text-foreground">
          {node.props.label}
        </label>
      </div>
    )
  }
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <label htmlFor={id} className="text-sm text-foreground">
        {node.props.label}
      </label>
      {node.type === 'input' ? (
        <Input
          id={id}
          value={String(value)}
          placeholder={node.props.placeholder}
          maxLength={node.props.maxLength}
          disabled={blocked}
          onChange={(event) => change(event.currentTarget.value)}
        />
      ) : (
        <Select value={String(value)} disabled={blocked} onValueChange={change}>
          <SelectTrigger id={id}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {node.props.options.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
    </div>
  )
}

function TreeNode({ node, tree, disabled, onAction }: NodeProps) {
  const id = useId()
  switch (node.type) {
    case 'column':
    case 'row':
      return (
        <div
          className={`flex min-w-0 ${node.type === 'column' ? 'flex-col' : 'flex-row'} ${UI_GAP_CLASSES[node.props.gap ?? 'sm']} ${UI_ALIGN_CLASSES[node.props.align ?? 'stretch']}`}
        >
          {node.children.map((child) => (
            <TreeNode
              key={child.id}
              node={child}
              tree={tree}
              disabled={disabled}
              onAction={onAction}
            />
          ))}
        </div>
      )
    case 'text':
      return (
        <p
          className={`text-sm ${UI_TONE_CLASSES[node.props.tone ?? 'default']} ${node.props.wrap === false ? 'whitespace-nowrap' : 'whitespace-pre-wrap break-words'}`}
        >
          {node.props.text}
        </p>
      )
    case 'heading': {
      const Heading = node.props.level === 1 ? 'h1' : node.props.level === 3 ? 'h3' : 'h2'
      const size =
        node.props.level === 1 ? 'text-xl' : node.props.level === 3 ? 'text-base' : 'text-lg'
      return (
        <Heading className={`font-semibold text-foreground ${size}`}>{node.props.text}</Heading>
      )
    }
    case 'button':
      return (
        <Button
          type="button"
          variant={node.props.variant ?? 'default'}
          disabled={disabled || node.props.disabled}
          onClick={() => {
            if (!disabled && !node.props.disabled) {
              onAction({
                nodeId: node.id,
                actionId: node.props.actionId,
                kind: 'click',
                value: null
              })
            }
          }}
        >
          {node.props.label}
        </Button>
      )
    case 'input':
    case 'toggle':
    case 'select':
      return (
        <DraftControl
          key={`${node.type}:${node.id}:${node.props.actionId}`}
          id={id}
          node={node}
          tree={tree}
          disabled={disabled}
          onAction={onAction}
        />
      )
    case 'list':
      return (
        <ul className="flex flex-col gap-2 text-sm text-foreground">
          {node.props.items.map((item) => (
            <li key={item.id}>
              <span>{item.text}</span>
              {item.description && <p className="text-muted-foreground">{item.description}</p>}
            </li>
          ))}
        </ul>
      )
    case 'divider':
      return <Separator orientation={node.props.orientation ?? 'horizontal'} decorative={false} />
  }
}

/** No IPC or plugin-defined DOM, style, class names or event handlers cross this boundary. */
export function UiTreeRenderer({ tree, disabled = false, onAction }: UiTreeRendererProps) {
  const { t } = useTranslation('plugins')
  const validated = useMemo(() => {
    try {
      return parseUiTree(tree)
    } catch {
      return null
    }
  }, [tree])
  useEffect(() => {
    if (!validated) {
      void logFrontendError({
        source: 'plugins.UiTreeRenderer',
        message: 'Plugin UI tree validation failed'
      })
    }
  }, [validated])
  if (!validated) {
    return (
      <p role="alert" className="text-sm text-destructive">
        {t('view.invalid')}
      </p>
    )
  }
  return (
    <TreeNode
      key={validated.viewId}
      node={validated.root}
      tree={validated}
      disabled={disabled}
      onAction={onAction}
    />
  )
}
