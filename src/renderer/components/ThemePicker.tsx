import { Check, Palette, Search, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useUpdateAppSettings } from '@/hooks/use-app-settings'
import { useEffectiveColorThemeId } from '@/hooks/use-color-theme'
import {
  COLOR_THEME_FAMILIES,
  getColorThemeDefinition,
  getPickerApplySettings,
  THEME_PICKER_ROWS,
  type ThemePickerRow
} from '@/lib/themes'
import { cn } from '@/lib/utils'
import { useThemePickerStore } from '@/stores/theme-picker-store'

function ThemeSwatches({ themeId }: { themeId: string }): React.JSX.Element {
  const palette = getColorThemeDefinition(themeId).dark.palette
  const colors = [palette.neutral, palette.primary, palette.accent, palette.success]

  return (
    <span className="flex items-center gap-0.5 shrink-0" aria-hidden="true">
      {colors.map((color) => (
        <span
          key={`${themeId}-${color}`}
          className="h-2.5 w-2.5 rounded-sm border border-border/70"
          style={{ backgroundColor: color }}
        />
      ))}
    </span>
  )
}

export function ThemePicker(): React.JSX.Element | null {
  const { t } = useTranslation('shell')
  const isOpen = useThemePickerStore((state) => state.isOpen)
  const highlightedThemeId = useThemePickerStore((state) => state.highlightedThemeId)
  const preview = useThemePickerStore((state) => state.preview)
  const cancel = useThemePickerStore((state) => state.cancel)
  const close = useThemePickerStore((state) => state.close)

  const effectiveThemeId = useEffectiveColorThemeId()
  const updateSettings = useUpdateAppSettings()

  const [query, setQuery] = useState('')
  const [focusIndex, setFocusIndex] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const previousQueryRef = useRef(query)

  const getRowLabel = useCallback(
    (row: ThemePickerRow): string =>
      row.variant === 'light'
        ? t('themes.lightVariant', { name: row.label.replace(/ Light$/, '') })
        : row.label,
    [t]
  )

  const filteredRows = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    if (!normalized) return THEME_PICKER_ROWS
    return THEME_PICKER_ROWS.filter(
      (row) =>
        getRowLabel(row).toLowerCase().includes(normalized) ||
        row.familyId.toLowerCase().includes(normalized) ||
        row.themeId.toLowerCase().includes(normalized)
    )
  }, [getRowLabel, query])

  const filteredFamilies = useMemo(() => {
    const familyIds = new Set(filteredRows.map((row) => row.familyId))
    return COLOR_THEME_FAMILIES.filter((family) => familyIds.has(family.familyId))
  }, [filteredRows])

  const flatFilteredRows = useMemo(() => {
    return filteredFamilies.flatMap((family) =>
      filteredRows.filter((row) => row.familyId === family.familyId)
    )
  }, [filteredFamilies, filteredRows])

  const confirmRow = useCallback(
    async (row: ThemePickerRow) => {
      const apply = getPickerApplySettings(row.themeId)
      await updateSettings({
        colorTheme: apply.colorTheme,
        appearanceMode: apply.appearanceMode
      })
      close()
      setQuery('')
    },
    [close, updateSettings]
  )

  const handleCancel = useCallback(() => {
    cancel()
    setQuery('')
  }, [cancel])

  const scrollFocusedIntoView = useCallback((index: number) => {
    const list = listRef.current
    if (!list) return
    const options = list.querySelectorAll<HTMLElement>('[data-theme-row]')
    const child = options[index]
    child?.scrollIntoView({ block: 'nearest' })
  }, [])

  const wasOpenRef = useRef(false)

  useEffect(() => {
    if (isOpen && !wasOpenRef.current) {
      const highlightedIndex = highlightedThemeId
        ? flatFilteredRows.findIndex((row) => row.themeId === highlightedThemeId)
        : 0
      setFocusIndex(highlightedIndex >= 0 ? highlightedIndex : 0)
    }
    wasOpenRef.current = isOpen

    if (!isOpen) return
    const frame = requestAnimationFrame(() => {
      inputRef.current?.focus()
    })
    return () => cancelAnimationFrame(frame)
  }, [flatFilteredRows, highlightedThemeId, isOpen])

  useEffect(() => {
    if (!isOpen) return
    scrollFocusedIntoView(focusIndex)
  }, [focusIndex, isOpen, scrollFocusedIntoView])

  useEffect(() => {
    if (!isOpen) {
      previousQueryRef.current = query
      return
    }

    if (previousQueryRef.current === query) return
    previousQueryRef.current = query

    const row = flatFilteredRows[0]
    if (row) {
      setFocusIndex(0)
      preview(row.themeId)
    }
  }, [flatFilteredRows, isOpen, preview, query])

  useEffect(() => {
    if (!isOpen) return

    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        handleCancel()
        return
      }

      if (event.key === 'ArrowDown') {
        event.preventDefault()
        const next = Math.min(focusIndex + 1, flatFilteredRows.length - 1)
        setFocusIndex(next)
        const row = flatFilteredRows[next]
        if (row) preview(row.themeId)
        return
      }

      if (event.key === 'ArrowUp') {
        event.preventDefault()
        const next = Math.max(focusIndex - 1, 0)
        setFocusIndex(next)
        const row = flatFilteredRows[next]
        if (row) preview(row.themeId)
        return
      }

      if (event.key === 'Enter') {
        event.preventDefault()
        const row =
          flatFilteredRows[focusIndex] ??
          flatFilteredRows.find((item) => item.themeId === highlightedThemeId)
        if (row) {
          void confirmRow(row)
        }
      }
    }

    window.addEventListener('keydown', handleKeyDown, { capture: true })
    return () => window.removeEventListener('keydown', handleKeyDown, { capture: true })
  }, [confirmRow, flatFilteredRows, focusIndex, handleCancel, highlightedThemeId, isOpen, preview])

  useEffect(() => {
    if (focusIndex >= flatFilteredRows.length) {
      setFocusIndex(Math.max(0, flatFilteredRows.length - 1))
    }
  }, [flatFilteredRows.length, focusIndex])

  if (!isOpen) return null

  let rowCounter = 0

  return (
    <div className="fixed inset-0 z-[120] pointer-events-none">
      <div
        className="absolute inset-0 pointer-events-auto"
        onMouseDown={(event) => {
          if (event.target === event.currentTarget) {
            handleCancel()
          }
        }}
        aria-hidden="true"
      />

      <section
        role="dialog"
        aria-modal="true"
        aria-label={t('themes.dialogLabel')}
        className="pointer-events-auto absolute bottom-3 left-12 top-3 flex w-[min(20rem,calc(100vw-2rem))] flex-col overflow-hidden rounded-md border border-border/80 bg-popover shadow-[0_12px_36px_hsl(var(--background)/0.65),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="flex items-center gap-2 border-b border-border/70 px-3 py-2">
          <Palette size={14} className="shrink-0 text-muted-foreground" aria-hidden="true" />
          <div className="min-w-0 flex-1">
            <h2 className="text-xs font-medium leading-none tracking-[-0.01em] text-foreground">
              {t('themes.title')}
            </h2>
            <p className="mt-0.5 text-2xs text-muted-foreground">{t('themes.hint')}</p>
          </div>
          <button
            type="button"
            onClick={handleCancel}
            className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            aria-label={t('themes.close')}
          >
            <X size={14} />
          </button>
        </header>

        <div className="border-b border-border/70 px-3 py-2">
          <div className="relative">
            <Search
              size={14}
              className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground"
              aria-hidden="true"
            />
            <input
              ref={inputRef}
              type="search"
              value={query}
              onChange={(event) => {
                setQuery(event.target.value)
                setFocusIndex(0)
              }}
              placeholder={t('themes.searchPlaceholder')}
              className="h-8 w-full rounded-md border border-input/80 bg-secondary/35 py-1.5 pl-8 pr-3 text-sm text-foreground outline-none transition-[border-color,background-color] placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:bg-secondary/50 focus-visible:ring-1 focus-visible:ring-ring/35"
              aria-label={t('themes.searchAria')}
            />
          </div>
        </div>

        <div
          ref={listRef}
          className="flex-1 overflow-y-auto p-1.5"
          role="listbox"
          aria-label={t('themes.listAria')}
        >
          {filteredFamilies.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-2 px-3 py-10 text-center">
              <div className="flex size-8 items-center justify-center rounded-md bg-secondary/50">
                <Search size={14} className="text-muted-foreground" />
              </div>
              <p className="text-xs font-medium text-foreground">{t('themes.empty')}</p>
            </div>
          ) : (
            filteredFamilies.map((family) => {
              const rows = filteredRows.filter((row) => row.familyId === family.familyId)
              return (
                <div key={family.familyId} className="mb-2 last:mb-0">
                  <p className="label-group px-2 pb-1 text-muted-foreground">{family.name}</p>
                  {rows.map((row) => {
                    const index = rowCounter
                    rowCounter += 1
                    const isApplied = row.themeId === effectiveThemeId
                    const isHighlighted =
                      row.themeId === highlightedThemeId ||
                      (highlightedThemeId === null && index === focusIndex)
                    const isFocused = index === focusIndex

                    return (
                      <button
                        key={row.themeId}
                        type="button"
                        role="option"
                        data-theme-row
                        aria-selected={isHighlighted}
                        className={cn(
                          'flex h-8 w-full items-center gap-2 rounded-sm px-2 text-left text-xs transition-colors',
                          isHighlighted || isFocused
                            ? 'bg-secondary text-foreground shadow-[inset_0_1px_0_0_hsl(var(--foreground)/0.04)] ring-1 ring-inset ring-primary/30'
                            : 'text-foreground/90 hover:bg-secondary/70'
                        )}
                        onMouseEnter={() => {
                          setFocusIndex(index)
                          preview(row.themeId)
                        }}
                        onFocus={() => {
                          setFocusIndex(index)
                          preview(row.themeId)
                        }}
                        onClick={() => {
                          void confirmRow(row)
                        }}
                      >
                        <ThemeSwatches themeId={row.themeId} />
                        <span className="flex-1 truncate font-medium">{getRowLabel(row)}</span>
                        {isApplied ? (
                          <Check
                            size={14}
                            className="shrink-0 text-primary"
                            aria-label={t('themes.applied')}
                          />
                        ) : null}
                      </button>
                    )
                  })}
                </div>
              )
            })
          )}
        </div>

        <footer className="border-t border-border/70 bg-secondary/20 px-3 py-1.5 text-2xs text-muted-foreground">
          {t('themes.cancel')} · {t('themes.apply')}
        </footer>
      </section>
    </div>
  )
}
