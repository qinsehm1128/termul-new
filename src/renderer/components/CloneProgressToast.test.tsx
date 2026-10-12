import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { GIT_CLONE_CANCELLED, type GitCloneProgress } from '@/lib/git-api'

const { mockToast } = vi.hoisted(() => {
  const toastFn = Object.assign(vi.fn(), {
    custom: vi.fn(),
    dismiss: vi.fn(),
    success: vi.fn(),
    error: vi.fn()
  })
  return { mockToast: toastFn }
})

vi.mock('sonner', () => ({ toast: mockToast }))

import {
  applyCloneProgress,
  CLONE_STALL_WARNING_MS,
  type CloneViewState,
  formatElapsed,
  runCloneWithProgress
} from './CloneProgressToast'

function progress(line: string, fields: Partial<GitCloneProgress> = {}): GitCloneProgress {
  return { line, phase: null, percent: null, transferred: null, speed: null, ...fields }
}

const receiving = (percent: number) =>
  progress(`Receiving objects:  ${percent}% (1/9), 22.25 MiB | 1.28 MiB/s`, {
    phase: 'Receiving objects',
    percent,
    transferred: '22.25 MiB',
    speed: '1.28 MiB/s'
  })

/** Start a clone whose promise the test settles, and render its card. */
function startClone() {
  let settle: { resolve: (path: string) => void; reject: (error: unknown) => void } | undefined
  let report: (progress: GitCloneProgress) => void = () => undefined
  const clone = vi.fn(
    (_cloneId: string, onProgress: (progress: GitCloneProgress) => void) =>
      new Promise<string>((resolve, reject) => {
        report = onProgress
        settle = { resolve, reject }
      })
  )
  const cancel = vi.fn().mockResolvedValue(true)
  const result = runCloneWithProgress({ repoName: 'TraceMemo', clone, cancel })
  const renderCard = mockToast.custom.mock.calls.at(-1)?.[0] as (id: string) => ReactNode
  render(<>{renderCard('id')}</>)
  return {
    result,
    clone,
    cancel,
    report: (p: GitCloneProgress) => act(() => report(p)),
    settle: () => settle!
  }
}

describe('applyCloneProgress', () => {
  const initial: CloneViewState = {
    startedAt: 0,
    lastProgressAt: 0,
    phase: null,
    percent: null,
    transferred: null,
    speed: null,
    log: [],
    cancelling: false
  }

  it('keeps one log line per phase as git redraws it', () => {
    let state = applyCloneProgress(initial, progress("Cloning into 'x'..."), 1)
    state = applyCloneProgress(state, receiving(10), 2)
    state = applyCloneProgress(state, receiving(24), 3)
    expect(state.log).toEqual([
      "Cloning into 'x'...",
      'Receiving objects:  24% (1/9), 22.25 MiB | 1.28 MiB/s'
    ])
    expect(state).toMatchObject({ phase: 'Receiving objects', percent: 24, lastProgressAt: 3 })
  })

  it('keeps the current phase through a plain line', () => {
    const state = applyCloneProgress(
      applyCloneProgress(initial, receiving(50), 1),
      progress('remote: Total 9 (delta 1)'),
      2
    )
    expect(state.phase).toBe('Receiving objects')
    expect(state.log.at(-1)).toBe('remote: Total 9 (delta 1)')
  })
})

describe('formatElapsed', () => {
  it('formats minutes and seconds', () => {
    expect(formatElapsed(0)).toBe('00:00')
    expect(formatElapsed(83_400)).toBe('01:23')
  })
})

describe('runCloneWithProgress', () => {
  beforeEach(() => {
    mockToast.mockClear()
    mockToast.custom.mockClear()
    mockToast.dismiss.mockClear()
    mockToast.success.mockClear()
    mockToast.error.mockClear()
  })

  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('shows the phase, percentage, size and speed as git reports them', async () => {
    const run = startClone()
    expect(screen.getByTestId('clone-progress-phase')).toHaveTextContent('Connecting')

    await run.report(receiving(24))

    expect(screen.getByTestId('clone-progress-phase')).toHaveTextContent('Receiving objects 24%')
    expect(screen.getByTestId('clone-progress-facts')).toHaveTextContent(
      '22.25 MiB · 1.28 MiB/s · 00:00 elapsed'
    )
    fireEvent.click(screen.getByTestId('clone-progress-log-toggle'))
    expect(screen.getByTestId('clone-progress-log')).toHaveTextContent(
      'Receiving objects: 24% (1/9)'
    )
  })

  it('resolves to the folder and reports success', async () => {
    const run = startClone()
    run.settle().resolve('/work/TraceMemo')
    await expect(run.result).resolves.toBe('/work/TraceMemo')
    expect(mockToast.success).toHaveBeenCalledWith('Cloned TraceMemo')
    expect(mockToast.dismiss).toHaveBeenCalled()
  })

  it('cancels the same clone it started and reports a cancellation, not a failure', async () => {
    const run = startClone()
    fireEvent.click(screen.getByTestId('clone-progress-cancel'))

    expect(run.cancel).toHaveBeenCalledWith(run.clone.mock.calls[0][0])
    expect(screen.getByTestId('clone-progress-cancel')).toBeDisabled()

    run.settle().reject(GIT_CLONE_CANCELLED)
    await expect(run.result).resolves.toBeNull()
    expect(mockToast).toHaveBeenCalledWith('Cancelled the clone of TraceMemo')
    expect(mockToast.error).not.toHaveBeenCalled()
  })

  it('reports git’s error when the clone fails', async () => {
    const run = startClone()
    run.settle().reject('fatal: repository not found')
    await expect(run.result).resolves.toBeNull()
    expect(mockToast.error).toHaveBeenCalledWith('Clone failed: fatal: repository not found')
  })

  it('warns once git has been silent long enough to look stuck', async () => {
    vi.useFakeTimers()
    const run = startClone()
    await run.report(receiving(3))
    expect(screen.queryByTestId('clone-progress-stalled')).not.toBeInTheDocument()

    act(() => {
      vi.advanceTimersByTime(CLONE_STALL_WARNING_MS + 1000)
    })
    expect(screen.getByTestId('clone-progress-stalled')).toHaveTextContent('No new progress for 31')

    await run.report(receiving(4))
    expect(screen.queryByTestId('clone-progress-stalled')).not.toBeInTheDocument()
  })
})
