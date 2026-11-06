import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger
} from '@/components/ui/context-menu'

/**
 * F5: locks the destructive-variant styling on `ContextMenuItem`
 * (ui/context-menu.tsx). Mirrors the deleted `ContextMenu.test.tsx`
 * `text-red-400` assertion — a destructive item must use the theme-token
 * `text-destructive` + `focus:bg-destructive/10`, never raw `red-*`.
 *
 * Radix's `MenuItem` must be used within a `Menu` context, so each test
 * renders the full `<ContextMenu>` tree, opens it via right-click, and
 * queries the portaled item via `findByRole('menuitem')`.
 */
describe('ContextMenuItem destructive variant (F5)', () => {
  function renderMenuTree(variant: 'default' | 'destructive' = 'default') {
    return render(
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <button type="button">trigger</button>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem variant={variant}>Delete</ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    )
  }

  async function openAndGetItem(): Promise<HTMLElement> {
    fireEvent.contextMenu(screen.getByText('trigger'))
    return screen.findByRole('menuitem')
  }

  it('applies text-destructive + focus:bg-destructive/10 when variant="destructive"', async () => {
    renderMenuTree('destructive')
    const item = await openAndGetItem()

    expect(item.className).toContain('text-destructive')
    expect(item.className).toContain('focus:bg-destructive/10')
    expect(item.className).toContain('focus:text-destructive')
    // No raw red-* classes — the destructive token is theme-driven.
    expect(item.className).not.toMatch(/text-red-/)
  })

  it('does NOT apply text-destructive for the default variant', async () => {
    renderMenuTree()
    const item = await openAndGetItem()

    expect(item.className).not.toContain('text-destructive')
  })

  it('does NOT apply text-destructive when variant is explicitly "default"', async () => {
    renderMenuTree('default')
    const item = await openAndGetItem()

    expect(item.className).not.toContain('text-destructive')
  })
})

describe('ContextMenuSubContent scrolling', () => {
  it('keeps a 40-project submenu height-bounded and vertically scrollable', async () => {
    const { ContextMenuSub, ContextMenuSubContent, ContextMenuSubTrigger } = await import(
      '@/components/ui/context-menu'
    )

    render(
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <button type="button">Group actions</button>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuSub>
            <ContextMenuSubTrigger>Add project</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              {Array.from({ length: 40 }, (_, index) => (
                <ContextMenuItem key={index}>Project {index + 1}</ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
        </ContextMenuContent>
      </ContextMenu>
    )

    fireEvent.contextMenu(screen.getByRole('button', { name: 'Group actions' }))
    const subTrigger = await screen.findByRole('menuitem', { name: 'Add project' })
    fireEvent.keyDown(subTrigger, { key: 'ArrowRight' })

    const submenu = await screen.findByRole('menu', { name: 'Add project' })
    expect(submenu.querySelectorAll('[role="menuitem"]')).toHaveLength(40)
    expect(submenu).toHaveClass(
      'max-h-[var(--radix-context-menu-content-available-height)]',
      'overflow-y-auto'
    )
    expect(submenu).not.toHaveClass('overflow-hidden')
  })
})
