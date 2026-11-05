import { useEffect, useMemo, useState } from 'react'
import type { TocHeading } from '@/hooks/use-toc-headings'

function getActiveHeadingForLine(headings: TocHeading[], lineNumber: number): string | undefined {
  let activeHeadingId: string | undefined

  for (const heading of headings) {
    if (heading.line === undefined) {
      continue
    }

    if (heading.line <= lineNumber) {
      activeHeadingId = heading.id
      continue
    }

    break
  }

  return activeHeadingId ?? headings[0]?.id
}

export function getActiveHeadingFromVisibleRange(
  headings: TocHeading[],
  visibleRange?: { startLine: number; endLine: number }
): string | undefined {
  if (!headings.length) {
    return undefined
  }

  if (!visibleRange) {
    return headings[0]?.id
  }

  return getActiveHeadingForLine(headings, visibleRange.startLine)
}

interface UseCodeMirrorActiveHeadingOptions {
  headings: TocHeading[]
  visibleRange?: { startLine: number; endLine: number }
}

export function useCodeMirrorActiveHeading({
  headings,
  visibleRange
}: UseCodeMirrorActiveHeadingOptions): string | undefined {
  return useMemo(
    () => getActiveHeadingFromVisibleRange(headings, visibleRange),
    [headings, visibleRange]
  )
}

interface UseBlockNoteActiveHeadingOptions {
  headings: TocHeading[]
  container: HTMLElement | null
  isEnabled?: boolean
}

export function useBlockNoteActiveHeading({
  headings,
  container,
  isEnabled = true
}: UseBlockNoteActiveHeadingOptions): string | undefined {
  // Only block/TOC identity affects observation. New arrays and text/level edits
  // must not disconnect the observer or reset a still-valid active heading.
  const headingIdentity = JSON.stringify(
    headings.map((heading) => [heading.blockId ?? heading.id, heading.id])
  )
  const headingEntries = useMemo(
    () => JSON.parse(headingIdentity) as [string, string][],
    [headingIdentity]
  )
  const [activeHeadingId, setActiveHeadingId] = useState<string | undefined>(headings[0]?.id)

  useEffect(() => {
    setActiveHeadingId((current) =>
      headingEntries.some(([, id]) => id === current) ? current : headingEntries[0]?.[1]
    )
  }, [headingEntries])

  useEffect(() => {
    if (!isEnabled || !container || !headingEntries.length) {
      return
    }

    const headingIds = new Set(headingEntries.map(([blockId]) => blockId))
    const blockIdToTocId = new Map(headingEntries)
    const visibleHeadings = new Map<string, number>()
    const observedElements = new Map<string, HTMLElement>()

    const updateActiveHeading = (): void => {
      const next = Array.from(visibleHeadings.entries())
        .sort((a, b) => a[1] - b[1])
        .map(([headingId]) => headingId)[0]

      if (next) {
        setActiveHeadingId(next)
      }
    }

    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          const element = entry.target as HTMLElement
          const blockId = element.dataset.id
          const headingId =
            blockId && observedElements.get(blockId) === element
              ? blockIdToTocId.get(blockId)
              : undefined

          if (!headingId) {
            return
          }

          if (entry.isIntersecting) {
            visibleHeadings.set(headingId, entry.boundingClientRect.top)
          } else {
            visibleHeadings.delete(headingId)
          }
        })

        updateActiveHeading()
      },
      {
        root: container,
        threshold: [0, 0.1, 0.25],
        rootMargin: '0px 0px -65% 0px'
      }
    )

    const reconcileElements = (): void => {
      const next = new Map<string, HTMLElement>()
      container
        .querySelectorAll<HTMLElement>('[data-node-type="blockContainer"][data-id]')
        .forEach((element) => {
          const id = element.dataset.id
          if (id && headingIds.has(id) && element.querySelector('[data-content-type="heading"]')) {
            next.set(id, element)
          }
        })
      observedElements.forEach((element, id) => {
        if (next.get(id) !== element) {
          observer.unobserve(element)
          observedElements.delete(id)
          const tocId = blockIdToTocId.get(id)
          if (tocId) visibleHeadings.delete(tocId)
        }
      })
      next.forEach((element, id) => {
        if (observedElements.get(id) !== element) {
          observedElements.set(id, element)
          observer.observe(element)
        }
      })
    }
    reconcileElements()
    // ProseMirror can replace/move a block DOM node without changing its ID.
    // Observe structural changes only, never TOC highlighting/class changes.
    const mutations = new MutationObserver(reconcileElements)
    mutations.observe(container, { childList: true, subtree: true })

    return () => {
      mutations.disconnect()
      observer.disconnect()
      visibleHeadings.clear()
      observedElements.clear()
    }
  }, [container, headingEntries, isEnabled])

  return activeHeadingId
}
