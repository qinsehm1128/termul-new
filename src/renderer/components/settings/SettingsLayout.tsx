import { Search, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { type SettingsSearchEntry, searchSettings } from '@/lib/settings-search'
import { cn } from '@/lib/utils'

/** A settings category shown in the left sidebar. */
export interface SettingsCategory {
  /** Stable id; also used as the scroll anchor target (`data-settings-section`). */
  id: string
  /** Sidebar label. */
  label: string
  /** Optional icon rendered before the label. */
  icon?: React.ReactNode
}

interface SettingsLayoutProps {
  /** Categories to render in the sidebar, in display order. */
  categories: SettingsCategory[]
  /** Flat search index across every category. */
  searchIndex: readonly SettingsSearchEntry[]
  /**
   * Section content. Each section must be wrapped so its root carries
   * `data-settings-section="<categoryId>"` — use {@link SettingsSection}.
   */
  children: React.ReactNode
  /** Optional extra content rendered at the bottom of the sidebar. */
  sidebarFooter?: React.ReactNode
}

/**
 * Wrapper for a single settings section. Tags the section with its category id
 * so {@link SettingsLayout} can scroll to it and track the active category via
 * scroll-spy. The `id` doubles as a stable DOM id (`settings-section-<id>`) for
 * anchor-based navigation.
 */
export function SettingsSection({
  id,
  children,
  className
}: {
  id: string
  children: React.ReactNode
  className?: string
}): React.JSX.Element {
  return (
    <section
      id={`settings-section-${id}`}
      data-settings-section={id}
      className={cn('scroll-mt-4', className)}
    >
      {children}
    </section>
  )
}

/**
 * Shared settings shell: a left sidebar listing categories with the active one
 * highlighted, a fuzzy search box, and a scrollable content area. Clicking a
 * category (or selecting a search result) scrolls the matching section into
 * view; scroll-spy keeps the active category in sync while the user scrolls.
 *
 * The content remains a single scrollable column (scroll-spy navigation rather
 * than show-one-at-a-time) so the existing save bar and unsaved-changes guard
 * in ProjectSettings keep working unchanged.
 */
export function SettingsLayout({
  categories,
  searchIndex,
  children,
  sidebarFooter
}: SettingsLayoutProps): React.JSX.Element {
  const { t } = useTranslation('settings')
  const contentRef = useRef<HTMLDivElement | null>(null)
  const [activeId, setActiveId] = useState<string | undefined>(categories[0]?.id)
  const [query, setQuery] = useState('')
  // While a programmatic scroll is in flight, suppress scroll-spy so the active
  // highlight follows the click target rather than intermediate sections.
  const programmaticScrollRef = useRef(false)
  const programmaticTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const results = useMemo(() => searchSettings(query, searchIndex), [query, searchIndex])
  const isSearching = query.trim().length > 0

  const scrollToSection = useCallback((id: string, anchorId?: string) => {
    const root = contentRef.current
    if (!root) return

    const target =
      (anchorId && root.querySelector<HTMLElement>(`#${CSS.escape(anchorId)}`)) ||
      root.querySelector<HTMLElement>(`[data-settings-section="${CSS.escape(id)}"]`)
    if (!target) return

    programmaticScrollRef.current = true
    if (programmaticTimerRef.current) clearTimeout(programmaticTimerRef.current)
    programmaticTimerRef.current = setTimeout(() => {
      programmaticScrollRef.current = false
    }, 600)

    setActiveId(id)
    target.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }, [])

  // Scroll-spy: highlight the section nearest the top of the viewport.
  useEffect(() => {
    const root = contentRef.current
    if (!root || categories.length === 0) return

    const sections = Array.from(root.querySelectorAll<HTMLElement>('[data-settings-section]'))
    if (sections.length === 0) return

    const visible = new Map<string, number>()

    const recompute = (): void => {
      if (programmaticScrollRef.current) return
      let topId: string | undefined
      let bestDistance = Number.POSITIVE_INFINITY
      // Pick the section whose top edge is closest to the viewport top (0),
      // not the smallest raw value — sections scrolled past have large negative
      // tops while still intersecting, and must not stay active.
      for (const [id, top] of visible) {
        const distance = Math.abs(top)
        if (distance < bestDistance) {
          bestDistance = distance
          topId = id
        }
      }
      if (topId) setActiveId(topId)
    }

    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const id = (entry.target as HTMLElement).dataset.settingsSection
          if (!id) continue
          if (entry.isIntersecting) {
            visible.set(id, entry.boundingClientRect.top)
          } else {
            visible.delete(id)
          }
        }
        recompute()
      },
      {
        root,
        threshold: [0, 0.1, 0.5],
        rootMargin: '0px 0px -70% 0px'
      }
    )

    for (const section of sections) observer.observe(section)

    return () => {
      observer.disconnect()
      visible.clear()
    }
    // Re-run when the set of categories changes (sections added/removed).
  }, [categories])

  useEffect(() => {
    return () => {
      if (programmaticTimerRef.current) clearTimeout(programmaticTimerRef.current)
    }
  }, [])

  const handleCategoryKeyDown = (
    event: React.KeyboardEvent<HTMLButtonElement>,
    index: number
  ): void => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      const dir = event.key === 'ArrowDown' ? 1 : -1
      const next = (index + dir + categories.length) % categories.length
      const sidebar = event.currentTarget.parentElement
      const buttons = sidebar?.querySelectorAll<HTMLButtonElement>('[data-category-button]')
      buttons?.[next]?.focus()
    }
  }

  return (
    <div className="flex-1 flex min-h-0 overflow-hidden">
      {/* Sidebar */}
      <aside className="flex w-56 flex-shrink-0 flex-col border-r border-border/70 bg-sidebar shadow-[inset_-1px_0_0_hsl(var(--background)/0.35)]">
        <div className="border-b border-border/70 p-2">
          <div className="relative">
            <Search
              size={14}
              className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground"
            />
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t('layout.searchPlaceholder')}
              aria-label={t('layout.searchAria')}
              className="h-8 w-full rounded-md border border-input/80 bg-secondary/35 pl-8 pr-8 text-xs text-foreground outline-none transition-[border-color,background-color] placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:bg-secondary/50 focus-visible:ring-1 focus-visible:ring-ring/35"
            />
            {query && (
              <button
                type="button"
                onClick={() => setQuery('')}
                aria-label={t('layout.clearSearch')}
                className="absolute right-1.5 top-1/2 flex size-6 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X size={14} />
              </button>
            )}
          </div>
        </div>

        <nav
          aria-label={t('layout.categoriesAria')}
          className="flex-1 space-y-0.5 overflow-y-auto p-1.5"
        >
          {isSearching ? (
            results.length === 0 ? (
              <div className="flex flex-col items-center gap-2 px-3 py-8 text-center">
                <div className="flex size-8 items-center justify-center rounded-md bg-secondary/50">
                  <Search size={14} className="text-muted-foreground" />
                </div>
                <p className="text-2xs text-muted-foreground">
                  {t('layout.noResults', { query: query.trim() })}
                </p>
              </div>
            ) : (
              results.map((result) => (
                <button
                  key={`${result.categoryId}-${result.label}`}
                  type="button"
                  onClick={() => scrollToSection(result.categoryId, result.anchorId)}
                  className="flex w-full flex-col items-start gap-0.5 rounded-sm px-2 py-1.5 text-left text-xs text-foreground transition-colors hover:bg-secondary focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  <span className="font-medium">{result.label}</span>
                  <span className="text-2xs text-muted-foreground">
                    {categories.find((c) => c.id === result.categoryId)?.label}
                  </span>
                </button>
              ))
            )
          ) : (
            categories.map((category, index) => {
              const isActive = category.id === activeId
              return (
                <button
                  key={category.id}
                  type="button"
                  data-category-button
                  aria-current={isActive ? 'true' : undefined}
                  onClick={() => scrollToSection(category.id)}
                  onKeyDown={(e) => handleCategoryKeyDown(e, index)}
                  className={cn(
                    'relative flex h-7 w-full items-center gap-2 rounded-sm px-2 text-left text-xs transition-colors before:absolute before:left-0 before:h-3.5 before:w-px before:rounded-full before:bg-primary before:opacity-0 before:transition-opacity focus:outline-none focus-visible:ring-1 focus-visible:ring-ring',
                    isActive
                      ? 'bg-secondary font-medium text-foreground shadow-[inset_0_1px_0_hsl(var(--foreground)/0.035)] before:opacity-100'
                      : 'text-muted-foreground hover:bg-secondary hover:text-foreground'
                  )}
                >
                  {category.icon && (
                    <span className="flex flex-shrink-0 items-center">{category.icon}</span>
                  )}
                  <span className="truncate">{category.label}</span>
                </button>
              )
            })
          )}
        </nav>

        {sidebarFooter && <div className="border-t border-border/70 p-2">{sidebarFooter}</div>}
      </aside>

      {/* Content */}
      <div
        ref={contentRef}
        className="min-w-0 flex-1 overflow-y-auto bg-background px-8 pb-32 pt-7"
      >
        <div className="mx-auto max-w-4xl space-y-8">{children}</div>
      </div>
    </div>
  )
}
