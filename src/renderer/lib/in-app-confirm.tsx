import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
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

export interface InAppPromptOptions {
  title: string
  message: string
  defaultValue?: string
  placeholder?: string
  confirmLabel?: string
  cancelLabel?: string
}

type PendingRequest =
  | ({ kind: 'confirm'; id: number; resolve: (confirmed: boolean) => void } & InAppConfirmOptions)
  | ({ kind: 'prompt'; id: number; resolve: (value: string | null) => void } & InAppPromptOptions)

let active: PendingRequest | null = null
let queue: PendingRequest[] = []
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

function settleConfirm(id: number, confirmed: boolean): void {
  if (!active || active.id !== id || active.kind !== 'confirm') return
  const { resolve } = active
  active = null
  resolve(confirmed)
  showNext()
}

function settlePrompt(id: number, value: string | null): void {
  if (!active || active.id !== id || active.kind !== 'prompt') return
  const { resolve } = active
  active = null
  resolve(value)
  showNext()
}

export function requestConfirm(options: InAppConfirmOptions): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    queue = [...queue, { kind: 'confirm', ...options, id: nextId++, resolve }]
    if (!active) showNext()
  })
}

export function requestPrompt(options: InAppPromptOptions): Promise<string | null> {
  return new Promise<string | null>((resolve) => {
    queue = [...queue, { kind: 'prompt', ...options, id: nextId++, resolve }]
    if (!active) showNext()
  })
}

/** Dismiss every pending request with `false` / `null`. Used on teardown and in tests. */
export function resetInAppConfirm(): void {
  const pending = active ? [active, ...queue] : queue
  active = null
  queue = []
  emit()
  for (const request of pending) {
    if (request.kind === 'confirm') request.resolve(false)
    else request.resolve(null)
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function getSnapshot(): PendingRequest | null {
  return active
}

/**
 * Renders the single visible confirmation or text prompt. Mount once in every
 * renderer root (`TauriApp.tsx` and `App.tsx`).
 *
 * `messageClassName="whitespace-pre-line"` keeps line breaks for callers that
 * pass line-separated copy; it does not change any other ConfirmDialog user.
 */
export function InAppConfirmHost(): React.JSX.Element {
  const pending = useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
  const [promptValue, setPromptValue] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (pending?.kind === 'prompt') {
      setPromptValue(pending.defaultValue ?? '')
      const frame = window.requestAnimationFrame(() => {
        inputRef.current?.focus()
        inputRef.current?.select()
      })
      return () => window.cancelAnimationFrame(frame)
    }
    return undefined
  }, [pending])

  const handleConfirm = (): void => {
    if (!pending) return
    if (pending.kind === 'confirm') {
      settleConfirm(pending.id, true)
      return
    }
    const trimmed = promptValue.trim()
    settlePrompt(pending.id, trimmed.length > 0 ? trimmed : null)
  }

  const handleCancel = (): void => {
    if (!pending) return
    if (pending.kind === 'confirm') settleConfirm(pending.id, false)
    else settlePrompt(pending.id, null)
  }

  return (
    <ConfirmDialog
      isOpen={pending !== null}
      title={pending?.title ?? ''}
      message={pending?.message ?? ''}
      confirmLabel={pending?.confirmLabel}
      cancelLabel={pending?.cancelLabel}
      variant={pending?.kind === 'confirm' ? (pending.variant ?? 'default') : 'default'}
      messageClassName="whitespace-pre-line"
      onConfirm={handleConfirm}
      onCancel={handleCancel}
    >
      {pending?.kind === 'prompt' ? (
        <input
          ref={inputRef}
          type="text"
          value={promptValue}
          placeholder={pending.placeholder}
          onChange={(e) => setPromptValue(e.target.value)}
          className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs text-foreground shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        />
      ) : null}
    </ConfirmDialog>
  )
}
