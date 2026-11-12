/**
 * The in-app confirm host backs every confirmation that must not use a native
 * dialog (a macOS NSAlert sheet self-deadlocks the main thread). These tests pin
 * the promise contract and the FIFO queue: one dialog at a time, every request
 * settles exactly once.
 */
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { InAppConfirmHost, requestConfirm, resetInAppConfirm } from './in-app-confirm'

async function findDialog(title: string): Promise<HTMLElement> {
  const heading = await screen.findByRole('heading', { name: title })
  const panel = heading.closest('[tabindex="-1"]')
  if (!(panel instanceof HTMLElement)) throw new Error(`confirm dialog "${title}" has no panel`)
  return panel
}

describe('InAppConfirmHost', () => {
  beforeEach(() => {
    resetInAppConfirm()
  })

  afterEach(() => {
    cleanup()
    resetInAppConfirm()
  })

  it('renders the requested copy and resolves true when confirmed', async () => {
    render(<InAppConfirmHost />)

    let answer: Promise<boolean> = Promise.resolve(false)
    act(() => {
      answer = requestConfirm({
        title: 'Safe update installation',
        message: 'The app window always restarts.',
        confirmLabel: 'Safe Install & Restart',
        cancelLabel: 'Not now'
      })
    })

    const panel = await findDialog('Safe update installation')
    expect(panel).toHaveTextContent('The app window always restarts.')
    fireEvent.click(within(panel).getByRole('button', { name: 'Safe Install & Restart' }))

    await expect(answer).resolves.toBe(true)
    await waitFor(() =>
      expect(
        screen.queryByRole('heading', { name: 'Safe update installation' })
      ).not.toBeInTheDocument()
    )
  })

  it('resolves false when cancelled and when Escape is pressed', async () => {
    render(<InAppConfirmHost />)

    let answer: Promise<boolean> = Promise.resolve(true)
    act(() => {
      answer = requestConfirm({ title: 'Force update', message: 'This ends 3 terminals.' })
    })
    let panel = await findDialog('Force update')
    fireEvent.click(within(panel).getByRole('button', { name: 'Cancel' }))
    await expect(answer).resolves.toBe(false)

    act(() => {
      answer = requestConfirm({ title: 'Force update', message: 'This ends 3 terminals.' })
    })
    panel = await findDialog('Force update')
    fireEvent.keyDown(window, { key: 'Escape' })
    await expect(answer).resolves.toBe(false)
  })

  it('uses the danger variant when requested', async () => {
    render(<InAppConfirmHost />)

    act(() => {
      void requestConfirm({
        title: 'Force update',
        message: 'This ends 3 terminals.',
        confirmLabel: 'Force Update',
        variant: 'danger'
      })
    })

    const panel = await findDialog('Force update')
    expect(within(panel).getByRole('button', { name: 'Force Update' })).toHaveClass(
      'bg-destructive'
    )
  })

  it('shows one dialog at a time and queues later requests FIFO', async () => {
    render(<InAppConfirmHost />)

    let first: Promise<boolean> = Promise.resolve(false)
    let second: Promise<boolean> = Promise.resolve(false)
    act(() => {
      first = requestConfirm({ title: 'First request', message: 'one' })
      second = requestConfirm({ title: 'Second request', message: 'two' })
    })

    const firstPanel = await findDialog('First request')
    expect(screen.queryByRole('heading', { name: 'Second request' })).not.toBeInTheDocument()

    fireEvent.click(within(firstPanel).getByRole('button', { name: 'Cancel' }))
    await expect(first).resolves.toBe(false)

    const secondPanel = await findDialog('Second request')
    expect(secondPanel).toHaveTextContent('two')
    fireEvent.click(within(secondPanel).getByRole('button', { name: 'Confirm' }))
    await expect(second).resolves.toBe(true)
  })

  it('keeps line breaks in a multi-line message', async () => {
    render(<InAppConfirmHost />)

    act(() => {
      void requestConfirm({ title: 'Impact', message: 'Line one\nLine two' })
    })

    const panel = await findDialog('Impact')
    const message = within(panel).getByText(/Line one/)
    expect(message).toHaveClass('whitespace-pre-line')
    expect(message.textContent).toBe('Line one\nLine two')
  })

  it('dismisses every pending request with false on reset', async () => {
    render(<InAppConfirmHost />)

    let first: Promise<boolean> = Promise.resolve(true)
    let second: Promise<boolean> = Promise.resolve(true)
    act(() => {
      first = requestConfirm({ title: 'First request', message: 'one' })
      second = requestConfirm({ title: 'Second request', message: 'two' })
    })
    await findDialog('First request')

    act(() => {
      resetInAppConfirm()
    })

    await expect(first).resolves.toBe(false)
    await expect(second).resolves.toBe(false)
  })
})
