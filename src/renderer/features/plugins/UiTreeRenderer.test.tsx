import type { UiNode, UiTree } from '@shared/types/plugin.types'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import { UiTreeRenderer } from './UiTreeRenderer'

const logFrontendError = vi.hoisted(() => vi.fn().mockResolvedValue(undefined))
vi.mock('@/lib/log-api', () => ({ logFrontendError }))
vi.mock('react-i18next', () => ({
  useTranslation: (namespace: string) => ({ t: (key: string) => `${namespace}:${key}` })
}))
const originalScrollIntoView = Object.getOwnPropertyDescriptor(
  HTMLElement.prototype,
  'scrollIntoView'
)
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', {
    configurable: true,
    value: vi.fn()
  })
})
afterAll(() => {
  if (originalScrollIntoView)
    Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', originalScrollIntoView)
  else Reflect.deleteProperty(HTMLElement.prototype, 'scrollIntoView')
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

const button: UiNode = { type: 'button', id: 'run', props: { label: 'Run', actionId: 'run' } }
const input: UiNode = {
  type: 'input',
  id: 'name',
  props: { label: 'Name', value: 'A', placeholder: 'Name here', maxLength: 40, actionId: 'name' }
}
const toggle: UiNode = {
  type: 'toggle',
  id: 'enabled',
  props: { label: 'Enabled', checked: false, actionId: 'enabled' }
}
const select: UiNode = {
  type: 'select',
  id: 'choice',
  props: {
    label: 'Choice',
    value: 'one',
    actionId: 'choice',
    options: [
      { label: 'One', value: 'one' },
      { label: 'Two', value: 'two' }
    ]
  }
}
function treeWith(...children: UiNode[]): UiTree {
  return {
    schemaVersion: 1,
    viewId: 'main',
    root: { type: 'column', id: 'root', props: { gap: 'md', align: 'stretch' }, children }
  }
}

describe('UiTreeRenderer', () => {
  it('renders all ten node types with host-owned theme classes', () => {
    const tree = treeWith(
      { type: 'heading', id: 'title', props: { text: 'Title', level: 1 } },
      { type: 'text', id: 'text', props: { text: 'Details', tone: 'muted', wrap: false } },
      { type: 'row', id: 'row', props: { gap: 'sm', align: 'center' }, children: [button, toggle] },
      input,
      select,
      {
        type: 'list',
        id: 'items',
        props: { items: [{ id: 'first', text: 'First item', description: 'Description' }] }
      },
      { type: 'divider', id: 'divider', props: { orientation: 'vertical' } }
    )
    const { container } = render(<UiTreeRenderer tree={tree} onAction={vi.fn()} />)
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Title')
    expect(screen.getByText('Details')).toHaveClass('text-muted-foreground', 'whitespace-nowrap')
    expect(screen.getByRole('button', { name: 'Run' })).toBeEnabled()
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveValue('A')
    expect(screen.getByRole('textbox')).toHaveAttribute('maxlength', '40')
    expect(screen.getByRole('textbox')).toHaveAttribute('placeholder', 'Name here')
    expect(screen.getByRole('switch', { name: 'Enabled' })).not.toBeChecked()
    expect(screen.getByRole('combobox', { name: 'Choice' })).toHaveTextContent('One')
    expect(screen.getByRole('listitem')).toHaveTextContent('First itemDescription')
    expect(screen.getByRole('separator')).toHaveAttribute('aria-orientation', 'vertical')
    expect(container.firstElementChild).toHaveClass('flex-col', 'gap-3', 'items-stretch')
    expect(screen.getByRole('button', { name: 'Run' }).parentElement).toHaveClass(
      'flex-row',
      'gap-2',
      'items-center'
    )
  })

  it('maps button, input and toggle actions immediately without IPC metadata', () => {
    const onAction = vi.fn()
    render(<UiTreeRenderer tree={treeWith(button, input, toggle)} onAction={onAction} />)
    fireEvent.click(screen.getByRole('button', { name: 'Run' }))
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'AB' } })
    fireEvent.click(screen.getByRole('switch'))
    expect(onAction.mock.calls).toEqual([
      [{ nodeId: 'run', actionId: 'run', kind: 'click', value: null }],
      [{ nodeId: 'name', actionId: 'name', kind: 'change', value: 'AB' }],
      [{ nodeId: 'enabled', actionId: 'enabled', kind: 'change', value: true }]
    ])
  })

  it('maps the themed Select option to its string value', async () => {
    const onAction = vi.fn()
    render(<UiTreeRenderer tree={treeWith(select)} onAction={onAction} />)
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('option', { name: 'Two' }))
    expect(onAction).toHaveBeenCalledExactlyOnceWith({
      nodeId: 'choice',
      actionId: 'choice',
      kind: 'change',
      value: 'two'
    })
  })

  it('retains a local draft across unchanged server snapshots, then accepts an authoritative value', () => {
    const onAction = vi.fn()
    const tree = treeWith(input)
    const { rerender } = render(<UiTreeRenderer tree={tree} onAction={onAction} />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'AB' } })
    rerender(<UiTreeRenderer tree={tree} onAction={(action) => onAction(action)} />)
    expect(screen.getByRole('textbox')).toHaveValue('AB')
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'ABC' } })
    expect(screen.getByRole('textbox')).toHaveValue('ABC')
    expect(onAction.mock.calls.map(([action]) => action.value)).toEqual(['AB', 'ABC'])
    rerender(<UiTreeRenderer tree={treeWith(input)} onAction={onAction} />)
    expect(screen.getByRole('textbox')).toHaveValue('ABC')
    rerender(
      <UiTreeRenderer
        tree={treeWith({ ...input, props: { ...input.props, value: 'Server' } })}
        onAction={onAction}
      />
    )
    expect(screen.getByRole('textbox')).toHaveValue('Server')
  })

  it('preserves newer typing when an older edit is acknowledged, including the next character', () => {
    const onAction = vi.fn()
    const snapshot = (value: string) => treeWith({ ...input, props: { ...input.props, value } })
    const { rerender } = render(<UiTreeRenderer tree={snapshot('')} onAction={onAction} />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'a' } })
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'ab' } })
    rerender(<UiTreeRenderer tree={snapshot('a')} onAction={onAction} />)
    expect(screen.getByRole('textbox')).toHaveValue('ab')
    const field = screen.getByRole('textbox') as HTMLInputElement
    fireEvent.change(field, { target: { value: `${field.value}c` } })
    expect(onAction.mock.calls.map(([action]) => action.value)).toEqual(['a', 'ab', 'abc'])
    rerender(<UiTreeRenderer tree={snapshot('ab')} onAction={onAction} />)
    expect(screen.getByRole('textbox')).toHaveValue('abc')
    rerender(<UiTreeRenderer tree={snapshot('abc')} onAction={onAction} />)
    expect(screen.getByRole('textbox')).toHaveValue('abc')
    rerender(<UiTreeRenderer tree={snapshot('authoritative')} onAction={onAction} />)
    expect(screen.getByRole('textbox')).toHaveValue('authoritative')
  })

  it('optimistically reverses rapid toggle clicks and preserves reversal across older acknowledgment', () => {
    const onAction = vi.fn()
    const snapshot = (checked: boolean) =>
      treeWith({ ...toggle, props: { ...toggle.props, checked } })
    const { rerender } = render(<UiTreeRenderer tree={snapshot(false)} onAction={onAction} />)
    fireEvent.click(screen.getByRole('switch'))
    expect(screen.getByRole('switch')).toBeChecked()
    fireEvent.click(screen.getByRole('switch'))
    expect(screen.getByRole('switch')).not.toBeChecked()
    expect(onAction.mock.calls.map(([action]) => action.value)).toEqual([true, false])
    // An unrelated snapshot still showing the old base is not an acknowledgment.
    rerender(<UiTreeRenderer tree={snapshot(false)} onAction={onAction} />)
    expect(screen.getByRole('switch')).not.toBeChecked()
    rerender(<UiTreeRenderer tree={snapshot(true)} onAction={onAction} />)
    expect(screen.getByRole('switch')).not.toBeChecked()
    rerender(<UiTreeRenderer tree={snapshot(false)} onAction={onAction} />)
    expect(screen.getByRole('switch')).not.toBeChecked()
    rerender(<UiTreeRenderer tree={snapshot(true)} onAction={onAction} />)
    expect(screen.getByRole('switch')).toBeChecked()
  })

  it('optimistically selects and reverses before an older selection acknowledgment', async () => {
    const onAction = vi.fn()
    const snapshot = (value: string) => treeWith({ ...select, props: { ...select.props, value } })
    const { rerender } = render(<UiTreeRenderer tree={snapshot('one')} onAction={onAction} />)
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('option', { name: 'Two' }))
    expect(screen.getByRole('combobox')).toHaveTextContent('Two')
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'ArrowDown' })
    fireEvent.click(await screen.findByRole('option', { name: 'One' }))
    expect(onAction.mock.calls.map(([action]) => action.value)).toEqual(['two', 'one'])
    rerender(<UiTreeRenderer tree={snapshot('two')} onAction={onAction} />)
    expect(screen.getByRole('combobox')).toHaveTextContent('One')
    rerender(<UiTreeRenderer tree={snapshot('one')} onAction={onAction} />)
    expect(screen.getByRole('combobox')).toHaveTextContent('One')
  })

  it.each([
    'invalid',
    'unmount',
    'view',
    'action'
  ])('clears pending drafts on %s replacement', (replacement) => {
    const onAction = vi.fn()
    const original = treeWith(input, toggle)
    const { rerender, unmount } = render(<UiTreeRenderer tree={original} onAction={onAction} />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'unsent' } })
    fireEvent.click(screen.getByRole('switch'))
    if (replacement === 'unmount') {
      unmount()
      render(<UiTreeRenderer tree={original} onAction={onAction} />)
    } else {
      const next =
        replacement === 'invalid'
          ? null
          : replacement === 'view'
            ? { ...original, viewId: 'different' }
            : treeWith(
                { ...input, props: { ...input.props, actionId: 'different' } },
                { ...toggle, props: { ...toggle.props, actionId: 'different' } }
              )
      rerender(<UiTreeRenderer tree={next} onAction={onAction} />)
      if (replacement === 'invalid')
        rerender(<UiTreeRenderer tree={original} onAction={onAction} />)
    }
    expect(screen.getByRole('textbox')).toHaveValue('A')
    expect(screen.getByRole('switch')).not.toBeChecked()
  })

  it.each(['panel', 'node'])('disables every interactive control at %s level', (mode) => {
    const onAction = vi.fn()
    const nodes = [button, input, toggle, select].map((node) =>
      mode === 'node' ? ({ ...node, props: { ...node.props, disabled: true } } as UiNode) : node
    )
    render(
      <UiTreeRenderer tree={treeWith(...nodes)} disabled={mode === 'panel'} onAction={onAction} />
    )
    for (const role of ['button', 'textbox', 'switch', 'combobox']) {
      const control = screen.getByRole(role)
      expect(control).toBeDisabled()
      fireEvent.click(control)
    }
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Blocked' } })
    expect(onAction).not.toHaveBeenCalled()
  })

  it.each([1, 2, 3] as const)('renders heading level %i', (level) => {
    render(
      <UiTreeRenderer
        tree={treeWith({ type: 'heading', id: 'heading', props: { text: 'Header', level } })}
        onAction={vi.fn()}
      />
    )
    expect(screen.getByRole('heading', { level })).toHaveTextContent('Header')
  })

  it('renders plugin text literally without HTML, scripts, images or handlers', () => {
    const malicious = '<img src=x onerror="alert(1)"><script>alert(1)</script>'
    const { container } = render(
      <UiTreeRenderer
        tree={treeWith(
          { type: 'text', id: 'text', props: { text: malicious } },
          { type: 'heading', id: 'heading', props: { text: malicious } },
          { type: 'button', id: 'button', props: { label: malicious, actionId: 'safe' } },
          {
            type: 'list',
            id: 'list',
            props: { items: [{ id: 'item', text: malicious, description: malicious }] }
          }
        )}
        onAction={vi.fn()}
      />
    )
    expect(screen.getAllByText(malicious)).toHaveLength(5)
    expect(container.querySelector('script, img, [onerror]')).toBeNull()
  })
})

function deepTree(): UiTree {
  let node: UiNode = button
  for (let depth = 0; depth <= 32; depth++) {
    node = { type: 'column', id: `level-${depth}`, props: {}, children: [node] }
  }
  return { schemaVersion: 1, viewId: 'main', root: node }
}

const invalidTrees: [string, unknown][] = [
  ['null', null],
  ['unknown node', treeWith({ ...button, type: 'script' } as unknown as UiNode)],
  [
    'extra style prop',
    treeWith({ ...button, props: { ...button.props, style: { color: 'red' } } } as UiNode)
  ],
  [
    'extra script prop',
    treeWith({ ...button, props: { ...button.props, script: 'alert(1)' } } as UiNode)
  ],
  ['extra className', treeWith({ ...button, className: 'fixed inset-0' } as UiNode)],
  ['tree extra HTML', { ...treeWith(button), html: '<b>bad</b>' }],
  [
    'invalid theme enum',
    treeWith({ type: 'text', id: 'bad', props: { text: 'Bad', tone: 'red' } } as unknown as UiNode)
  ],
  [
    'invalid prop type',
    treeWith({ ...input, props: { ...input.props, value: 3 } } as unknown as UiNode)
  ],
  ['duplicate id', treeWith(button, button)],
  ['depth limit', deepTree()],
  [
    'node limit',
    treeWith(
      ...Array.from({ length: 1024 + 1 }, (_, index) => ({ ...button, id: `node-${index}` }))
    )
  ],
  ['text limit', treeWith({ type: 'text', id: 'long', props: { text: 'x'.repeat(8192 + 1) } })]
]

describe('whole-tree validation boundary', () => {
  it.each(invalidTrees)('rejects %s before rendering any valid siblings', (_name, invalid) => {
    const onAction = vi.fn()
    render(<UiTreeRenderer tree={invalid} onAction={onAction} />)
    expect(screen.getByRole('alert')).toHaveTextContent('plugins:view.invalid')
    expect(logFrontendError).toHaveBeenCalledExactlyOnceWith({
      source: 'plugins.UiTreeRenderer',
      message: 'Plugin UI tree validation failed'
    })
    expect(screen.queryByRole('button')).not.toBeInTheDocument()
    expect(onAction).not.toHaveBeenCalled()
  })

  it('does not partially render a valid sibling before an invalid child', () => {
    render(
      <UiTreeRenderer
        tree={treeWith(button, { type: 'iframe', id: 'bad', props: {} } as unknown as UiNode)}
        onAction={vi.fn()}
      />
    )
    expect(screen.getByRole('alert')).toBeInTheDocument()
    expect(screen.queryByText('Run')).not.toBeInTheDocument()
  })
})
