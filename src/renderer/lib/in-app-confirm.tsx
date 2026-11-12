import { useSyncExternalStore } from 'react'
import { ConfirmDialog } from '@/components/ConfirmDialog'

/**
 * Promise-based in-app confirmation.
 *
 * Why this exists: the native `confirm` from `@tauri-apps/plugin-dialog`
 * (tauri-plugin-dialog → rfd → `NSAlert beginSheetModalForWindow`) self-deadlocks
 * the macOS main thread when a sheet animation drives a nested runloop while tao
 * already holds its event-loop mutex (hang report: handle_user_message →
 * show_message_dialog → nested runloop → CA flush → handle_nonuser_event →
 * Mutex::lock). Install/restart confirmations therefore must be rendered by
 * React, not by the native dialog.
 *
 * Call sites that live outside the React tree (sonner toast actions) cannot hold
 * dialog state themselves, so they call `requestConfirm()` and await the answer.
 * `<InAppConfirmHost />` is mounted once per renderer root and renders the
 * project's `ConfirmDialog` from this store.
 *
 * Concurrency: requests are queued FIFO. Only one dialog is visible at a time;
 * the next queued request is shown once the current one is answered. Every
 * request settles exactly once with `true` (confirm) or `false` (cancel,
 * Escape, backdrop click).
 */

export interface InAppConfirmOptions {
  title: string
  message: string
  confirmLabel?: string
  cancelLabel?: string
  variant?: 'default' | 'danger'
}

interface PendingConfirm extends InAppConfirmOptions {
  id: number
  resolve: (confirmed: boolean) => void
}

let active: PendingConfirm | null = null
let queue: PendingConfirm[] = []
let nextId = 1
const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

function showNext(): void {
  active = queue[0] ?? null
  queue = queue.slice(1)
  emit()
}

function settle(id: number, confirmed: boolean): void {
  if (!active || active.id !== id) return
  const { resolve } = active
  active = null
  resolve(confirmed)
  showNext()
}

export function requestConfirm(options: InAppConfirmOptions): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    queue = [...queue, { ...options, id: nextId++, resolve }]
    if (!active) showNext()
  })
}

/** Dismiss every pending request with `false`. Used on teardown and in tests. */
export function resetInAppConfirm(): void {
  const pending = active ? [active, ...queue] : queue
  active = null
  queue = []
  emit()
  for (const request of pending) request.resolve(false)
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function getSnapshot(): PendingConfirm | null {
  return active
}

/**
 * Renders the single visible confirmation. Mount once in every renderer root
 * (`TauriApp.tsx` and `App.tsx`).
 *
 * `messageClassName="whitespace-pre-line"` keeps line breaks for callers that
 * pass line-separated copy; it does not change any other ConfirmDialog user.
 */
export function InAppConfirmHost(): React.JSX.Element {
  const pending = useSyncExternalStore(subscribe, getSnapshot, getSnapshot)

  return (
    <ConfirmDialog
      isOpen={pending !== null}
      title={pending?.title ?? ''}
      message={pending?.message ?? ''}
      confirmLabel={pending?.confirmLabel}
      cancelLabel={pending?.cancelLabel}
      variant={pending?.variant ?? 'default'}
      messageClassName="whitespace-pre-line"
      onConfirm={() => {
        if (pending) settle(pending.id, true)
      }}
      onCancel={() => {
        if (pending) settle(pending.id, false)
      }}
    />
  )
}
