import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { PageTabs, PageTabsContent, PageTabsList, PageTabsTrigger } from './page-tabs'

afterEach(() => cleanup())

describe('PageTabs', () => {
  it('shows one section at a time and keeps the hidden one mounted', () => {
    render(
      <PageTabs defaultValue="general">
        <PageTabsList aria-label="Sections">
          <PageTabsTrigger value="general">General</PageTabsTrigger>
          <PageTabsTrigger value="advanced">Advanced</PageTabsTrigger>
        </PageTabsList>
        <PageTabsContent value="general">
          <input aria-label="Name" />
        </PageTabsContent>
        <PageTabsContent value="advanced">advanced section</PageTabsContent>
      </PageTabs>
    )
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'half typed' } })

    // Radix activates a tab on mouse down.
    fireEvent.mouseDown(screen.getByRole('tab', { name: 'Advanced' }), { button: 0 })

    expect(screen.getByRole('tab', { name: 'Advanced' }).getAttribute('aria-selected')).toBe('true')
    expect(screen.getByText('advanced section').getAttribute('data-state')).toBe('active')
    const name = screen.getByLabelText('Name') as HTMLInputElement
    expect(name.closest('[role="tabpanel"]')?.getAttribute('data-state')).toBe('inactive')
    expect(name.value).toBe('half typed')
  })
})
