import { act, fireEvent, render } from '@testing-library/react'
import type { HTMLAttributes } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useBlockNote } from '@/hooks/use-blocknote'
import { flushEditorContent } from '@/lib/editor-content-flush'
import { MarkdownEditor } from './MarkdownEditor'

vi.mock('@blocknote/react', () => ({
  BlockNoteViewRaw: () => <div data-testid="blocknote-view" />
}))

vi.mock('@/hooks/use-blocknote', () => ({
  useBlockNote: vi.fn()
}))

const tocState = vi.hoisted(() => ({
  isVisible: false,
  setWidth: vi.fn(),
  onResize: undefined as ((size: number, prevSize: number | undefined) => void) | undefined,
  onDragging: undefined as ((dragging: boolean) => void) | undefined,
  getLayout: vi.fn(() => [80, 20]),
  setLayout: vi.fn()
}))

vi.mock('@/components/ui/resizable', async () => {
  const React = await import('react')
  return {
    ResizablePanelGroup: React.forwardRef(({ children }: { children: React.ReactNode }, ref) => {
      React.useImperativeHandle(ref, () => ({
        getLayout: tocState.getLayout,
        setLayout: tocState.setLayout
      }))
      return <div>{children}</div>
    }),
    ResizablePanel: ({
      children,
      onResize
    }: {
      children: React.ReactNode
      onResize?: (size: number, prevSize: number | undefined) => void
    }) => {
      React.useLayoutEffect(() => {
        if (onResize) {
          tocState.onResize = onResize
          onResize(20, undefined)
        }
      }, [onResize])
      return <div>{children}</div>
    },
    ResizableHandle: ({
      onDragging,
      ...props
    }: HTMLAttributes<HTMLDivElement> & {
      onDragging?: (dragging: boolean) => void
    }) => {
      tocState.onDragging = onDragging
      return <div {...props} data-testid="toc-resize-handle" />
    }
  }
})

vi.mock('@/stores/toc-settings-store', () => ({
  useTocSettingsStore: (selector: (state: unknown) => unknown) =>
    selector({
      isLoaded: true,
      loadFailed: false,
      settings: {
        isVisible: tocState.isVisible,
        maxHeadingLevel: 3,
        width: 280
      },
      setWidth: tocState.setWidth
    })
}))

type BlockNoteOptions = {
  filePath: string
  initialMarkdown: string
  onChange: (markdown: string) => void
}

const FM_CONTENT = `---
title: Spec
status: draft
---
# Heading

Body text.
`

describe('MarkdownEditor frontmatter strip/rejoin/flush', () => {
  let capturedOptions: BlockNoteOptions
  const replaceContent = vi.fn()
  const capturePendingContent = vi.fn(async () => '# Heading\n\nBody text.\n')
  const flushPendingContent = vi.fn(async () => {
    capturedOptions.onChange('# Heading\n\nBody text.\n')
  })

  beforeEach(() => {
    vi.clearAllMocks()
    tocState.isVisible = false
    tocState.onResize = undefined
    tocState.setLayout.mockImplementation((layout: number[]) => {
      // react-resizable-panels notifies synchronously, including group clamping.
      tocState.onResize?.(Math.min(25, layout[1]), 20)
    })
    capturePendingContent.mockResolvedValue('# Heading\n\nBody text.\n')
    flushPendingContent.mockImplementation(async () => {
      capturedOptions.onChange('# Heading\n\nBody text.\n')
    })

    vi.mocked(useBlockNote).mockImplementation((options: BlockNoteOptions) => {
      capturedOptions = options
      return {
        editor: {} as never,
        replaceContent,
        flushPendingContent,
        capturePendingContent,
        getHeadings: () => [],
        scrollToBlock: vi.fn(),
        subscribeDocumentChange: () => () => {},
        getDocumentVersion: () => 0
      }
    })
  })

  it('does not persist mount or programmatic TOC layout, but persists user resizing', () => {
    tocState.isVisible = true
    render(
      <MarkdownEditor filePath="/docs/spec.md" content={FM_CONTENT} isVisible onChange={vi.fn()} />
    )
    expect(tocState.setLayout).toHaveBeenCalled()
    expect(tocState.setWidth).not.toHaveBeenCalled()

    // Library constraint adjustments can report a previous size too.
    act(() => tocState.onResize?.(30, 25))
    expect(tocState.setWidth).not.toHaveBeenCalled()
    act(() => {
      tocState.onDragging?.(true)
      tocState.onResize?.(30, 25)
    })
    expect(tocState.setWidth).toHaveBeenCalledExactlyOnceWith(300)
    tocState.setWidth.mockClear()
    act(() => tocState.onResize?.(28, 30))
    expect(tocState.setWidth).not.toHaveBeenCalled()
    act(() => tocState.onDragging?.(false))
    act(() => tocState.onResize?.(35, 28))
    expect(tocState.setWidth).not.toHaveBeenCalled()
  })

  it('keeps Properties and BlockNote inside the scoped document shell', () => {
    const { getByLabelText, getByTestId } = render(
      <MarkdownEditor filePath="/docs/spec.md" content={FM_CONTENT} isVisible onChange={vi.fn()} />
    )

    const scrollRoot = getByTestId('blocknote-view').closest('.markdown-editor')
    const documentShell = getByTestId('blocknote-view').closest('.markdown-editor-document')

    expect(scrollRoot).toBeTruthy()
    expect(documentShell).toBeTruthy()
    expect(documentShell?.classList.contains('flex')).toBe(true)
    expect(documentShell?.classList.contains('flex-col')).toBe(true)
    expect(documentShell?.contains(getByLabelText('Properties'))).toBe(true)
  })

  it('passes body-only into useBlockNote and rejoins FM on body onChange', () => {
    const onChange = vi.fn()
    render(
      <MarkdownEditor filePath="/docs/spec.md" content={FM_CONTENT} isVisible onChange={onChange} />
    )

    expect(capturedOptions.initialMarkdown).toBe('# Heading\n\nBody text.\n')
    expect(capturedOptions.initialMarkdown.startsWith('---')).toBe(false)

    act(() => {
      capturedOptions.onChange('# Heading\n\nEdited body.\n')
    })

    expect(onChange).toHaveBeenCalledTimes(1)
    const full = onChange.mock.calls[0]?.[0] as string
    expect(full.startsWith('---\n')).toBe(true)
    expect(full).toContain('title: Spec')
    expect(full).toContain('status: draft')
    expect(full).toContain('# Heading\n\nEdited body.\n')
  })

  it('flush path emits rejoined full file including frontmatter', async () => {
    const onChange = vi.fn()
    render(
      <MarkdownEditor filePath="/docs/spec.md" content={FM_CONTENT} isVisible onChange={onChange} />
    )

    await act(async () => {
      await flushEditorContent('/docs/spec.md')
    })

    expect(flushPendingContent).toHaveBeenCalled()
    expect(onChange).toHaveBeenCalled()
    const full = onChange.mock.calls.at(-1)?.[0] as string
    expect(full).toContain('title: Spec')
    expect(full).toContain('# Heading\n\nBody text.\n')
  })

  it('Properties edit captures body once and emits rejoined FM without replaceContent', async () => {
    const onChange = vi.fn()
    const { getByLabelText } = render(
      <MarkdownEditor filePath="/docs/spec.md" content={FM_CONTENT} isVisible onChange={onChange} />
    )

    const titleInput = getByLabelText('title')
    await act(async () => {
      titleInput.focus()
      fireEvent.change(titleInput, { target: { value: 'Updated' } })
      fireEvent.blur(titleInput)
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(capturePendingContent).toHaveBeenCalled()
    expect(onChange).toHaveBeenCalled()
    const full = onChange.mock.calls.at(-1)?.[0] as string
    expect(full).toContain('title: Updated')
    expect(full).toContain('# Heading\n\nBody text.\n')
    // FM-only edit must not treat store echo as external reload
    expect(replaceContent).not.toHaveBeenCalled()
  })

  it('verbatim store echo clears pending local emits so later external reloads apply', () => {
    const onChange = vi.fn()
    const { rerender } = render(
      <MarkdownEditor filePath="/docs/spec.md" content={FM_CONTENT} isVisible onChange={onChange} />
    )

    act(() => {
      capturedOptions.onChange('# Heading\n\nEdited body.\n')
    })

    expect(onChange).toHaveBeenCalledTimes(1)
    const emitted = onChange.mock.calls[0]?.[0] as string
    expect(emitted).toBe(`---
title: Spec
status: draft
---
# Heading

Edited body.
`)
    expect(replaceContent).not.toHaveBeenCalled()

    // Parent echoes the emitted full file back as the content prop (verbatim).
    rerender(
      <MarkdownEditor filePath="/docs/spec.md" content={emitted} isVisible onChange={onChange} />
    )
    expect(replaceContent).not.toHaveBeenCalled()

    const external = `---
title: Spec
status: draft
---
# Heading

External reload.
`
    rerender(
      <MarkdownEditor filePath="/docs/spec.md" content={external} isVisible onChange={onChange} />
    )

    expect(replaceContent).toHaveBeenCalledTimes(1)
    expect(replaceContent).toHaveBeenCalledWith('# Heading\n\nExternal reload.\n')
  })
})
