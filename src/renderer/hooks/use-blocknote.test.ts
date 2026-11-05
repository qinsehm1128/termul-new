import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useBlockNote } from './use-blocknote'

const { editor, changeListeners } = vi.hoisted(() => {
  const changeListeners = new Set<() => void>()
  const editor = {
    document: [] as Array<Record<string, unknown>>,
    tryParseMarkdownToBlocks: vi.fn(),
    replaceBlocks: vi.fn(),
    blocksToMarkdownLossy: vi.fn(),
    onChange: vi.fn()
  }
  return { editor, changeListeners }
})

vi.mock('@blocknote/code-block', () => ({ codeBlockOptions: {} }))
vi.mock('@blocknote/core', () => ({
  BlockNoteEditor: { create: () => editor },
  BlockNoteSchema: { create: vi.fn() },
  createCodeBlockSpec: vi.fn(),
  createExtension: () => () => ({}),
  defaultBlockSpecs: {}
}))
vi.mock('@/components/editor/mermaid-block-spec', () => ({ mermaidBlockSpec: vi.fn() }))
vi.mock('@/lib/editor-save', () => ({ requestSaveEditorFile: vi.fn() }))
vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: () => false,
  resolveTauriAssetPath: vi.fn()
}))

beforeEach(() => {
  vi.useFakeTimers()
  vi.clearAllMocks()
  changeListeners.clear()
  editor.document = []
  editor.tryParseMarkdownToBlocks.mockReset().mockReturnValue([])
  editor.blocksToMarkdownLossy.mockReset().mockResolvedValue('# Edited')
  editor.onChange.mockImplementation((listener: () => void) => {
    changeListeners.add(listener)
    return () => changeListeners.delete(listener)
  })
  editor.replaceBlocks.mockImplementation((_, blocks: Array<Record<string, unknown>>) => {
    editor.document = blocks
    for (const listener of changeListeners) listener()
  })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('useBlockNote document changes', () => {
  it('signals subscribers after asynchronous initial markdown replacement', async () => {
    let finishParse!: (blocks: Array<Record<string, unknown>>) => void
    editor.tryParseMarkdownToBlocks.mockReturnValue(
      new Promise<Array<Record<string, unknown>>>((resolve) => {
        finishParse = resolve
      })
    )
    const { result } = renderHook(() =>
      useBlockNote({ filePath: '/note.md', initialMarkdown: '# Loaded', onChange: vi.fn() })
    )
    const store = result.current
    const listener = vi.fn()
    store.subscribeDocumentChange(listener)
    expect(store.getDocumentVersion()).toBe(0)

    await act(async () => {
      finishParse([{ id: 'loaded', type: 'heading', props: { level: 1 }, content: [] }])
    })

    expect(editor.replaceBlocks).toHaveBeenCalledTimes(1)
    expect(listener).toHaveBeenCalledTimes(1)
    expect(store.getDocumentVersion()).toBe(1)
  })

  it('publishes each real edit immediately, before the 300ms markdown debounce', async () => {
    const onChange = vi.fn()
    const { result } = renderHook(() =>
      useBlockNote({ filePath: '/note.md', initialMarkdown: '', onChange })
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(20)
    })
    const initialVersion = result.current.getDocumentVersion()
    const observedVersions: number[] = []
    result.current.subscribeDocumentChange(() => {
      observedVersions.push(result.current.getDocumentVersion())
    })

    act(() => {
      for (const listener of changeListeners) listener()
    })
    expect(observedVersions).toEqual([initialVersion + 1])
    expect(editor.blocksToMarkdownLossy).not.toHaveBeenCalled()
    expect(onChange).not.toHaveBeenCalled()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(100)
      for (const listener of changeListeners) listener()
    })
    expect(observedVersions).toEqual([initialVersion + 1, initialVersion + 2])
    await act(async () => {
      await vi.advanceTimersByTimeAsync(299)
    })
    expect(onChange).not.toHaveBeenCalled()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1)
    })
    expect(editor.blocksToMarkdownLossy).toHaveBeenCalledTimes(1)
    expect(onChange).toHaveBeenCalledExactlyOnceWith('# Edited')
    expect(result.current.getDocumentVersion()).toBe(initialVersion + 2)
  })

  it.each([
    'stale-first',
    'latest-first'
  ])('does not publish a superseded async parse (%s)', async (completionOrder) => {
    let finishStale!: (blocks: Array<Record<string, unknown>>) => void
    let finishLatest!: (blocks: Array<Record<string, unknown>>) => void
    editor.tryParseMarkdownToBlocks
      .mockReturnValueOnce(
        new Promise<Array<Record<string, unknown>>>((resolve) => {
          finishStale = resolve
        })
      )
      .mockReturnValueOnce(
        new Promise<Array<Record<string, unknown>>>((resolve) => {
          finishLatest = resolve
        })
      )
    const onChange = vi.fn()
    const { result } = renderHook(() =>
      useBlockNote({ filePath: '/note.md', initialMarkdown: '# Stale', onChange })
    )
    const listener = vi.fn()
    result.current.subscribeDocumentChange(listener)
    let latestReplacement!: Promise<void>
    act(() => {
      latestReplacement = result.current.replaceContent('# Latest')
    })
    const latestBlocks = [
      {
        id: 'latest',
        type: 'heading',
        props: { level: 1 },
        content: [{ type: 'text', text: 'Latest', styles: {} }]
      }
    ]
    const completeStale = async () => {
      await act(async () => {
        finishStale([{ id: 'stale', type: 'paragraph', content: [] }])
      })
    }
    const completeLatest = async () => {
      await act(async () => {
        finishLatest(latestBlocks)
        await latestReplacement
      })
    }
    if (completionOrder === 'stale-first') {
      await completeStale()
      expect(listener).not.toHaveBeenCalled()
      expect(result.current.getDocumentVersion()).toBe(0)
      expect(editor.replaceBlocks).not.toHaveBeenCalled()
      await completeLatest()
    } else {
      await completeLatest()
      await completeStale()
    }

    expect(editor.document).toEqual(latestBlocks)
    expect(editor.replaceBlocks).toHaveBeenCalledTimes(1)
    expect(listener).toHaveBeenCalledTimes(1)
    expect(result.current.getDocumentVersion()).toBe(1)
    expect(result.current.getHeadings()).toEqual([
      { id: 'latest', blockId: 'latest', level: 1, text: 'Latest' }
    ])
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500)
    })
    expect(onChange).not.toHaveBeenCalled()
  })

  it('publishes explicit replaceContent once and suppresses its editor change event', async () => {
    const onChange = vi.fn()
    const { result } = renderHook(() =>
      useBlockNote({ filePath: '/note.md', initialMarkdown: '', onChange })
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(20)
    })
    const initialVersion = result.current.getDocumentVersion()
    const listener = vi.fn()
    result.current.subscribeDocumentChange(listener)
    editor.tryParseMarkdownToBlocks.mockResolvedValueOnce([
      { id: 'replacement', type: 'paragraph', content: [] }
    ])

    await act(async () => {
      await result.current.replaceContent('replacement')
      await vi.advanceTimersByTimeAsync(500)
    })

    expect(listener).toHaveBeenCalledTimes(1)
    expect(result.current.getDocumentVersion()).toBe(initialVersion + 1)
    expect(onChange).not.toHaveBeenCalled()
  })

  it('keeps store callbacks and editor subscription stable across new parent props', async () => {
    const originalOnChange = vi.fn()
    const newOnChange = vi.fn()
    const { result, rerender } = renderHook((options) => useBlockNote(options), {
      initialProps: { filePath: '/note.md', initialMarkdown: '', onChange: originalOnChange }
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(20)
    })
    const { subscribeDocumentChange, getDocumentVersion } = result.current
    const listener = vi.fn()
    const unsubscribe = subscribeDocumentChange(listener)
    rerender({ filePath: '/other.md', initialMarkdown: '', onChange: newOnChange })

    expect(result.current.subscribeDocumentChange).toBe(subscribeDocumentChange)
    expect(result.current.getDocumentVersion).toBe(getDocumentVersion)
    expect(editor.onChange).toHaveBeenCalledTimes(1)
    expect(changeListeners.size).toBe(1)
    act(() => {
      for (const callback of changeListeners) callback()
    })
    expect(listener).toHaveBeenCalledTimes(1)
    unsubscribe()
    unsubscribe()
    act(() => {
      for (const callback of changeListeners) callback()
    })
    expect(listener).toHaveBeenCalledTimes(1)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300)
    })
    expect(originalOnChange).not.toHaveBeenCalled()
    expect(newOnChange).toHaveBeenCalledExactlyOnceWith('# Edited')
  })

  it('unregisters editor and document listeners and clears pending markdown on unmount', async () => {
    const onChange = vi.fn()
    const { result, unmount } = renderHook(() =>
      useBlockNote({ filePath: '/note.md', initialMarkdown: '', onChange })
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(20)
    })
    const listener = vi.fn()
    result.current.subscribeDocumentChange(listener)
    act(() => {
      for (const callback of changeListeners) callback()
    })
    expect(listener).toHaveBeenCalledTimes(1)
    unmount()
    expect(changeListeners.size).toBe(0)
    await act(async () => {
      await result.current.replaceContent('after unmount')
      await vi.advanceTimersByTimeAsync(500)
    })
    expect(listener).toHaveBeenCalledTimes(1)
    expect(onChange).not.toHaveBeenCalled()
  })
})
