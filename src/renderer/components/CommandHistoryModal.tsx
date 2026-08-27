import { AnimatePresence, motion } from 'framer-motion'
import { Clock, History, Terminal, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Virtuoso, type VirtuosoHandle } from 'react-virtuoso'
import { ConfirmDialog } from '@/components/ConfirmDialog'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from '@/components/ui/select'
import { formatDate } from '@/i18n/format'
import { cn } from '@/lib/utils'
import type { CommandHistoryEntry } from '@/stores/command-history-store'

type FilterMode = 'this-project' | 'all-projects'

interface CommandHistoryModalProps {
  isOpen: boolean
  onClose: () => void
  entries: CommandHistoryEntry[]
  allEntries: CommandHistoryEntry[]
  onSelectCommand: (command: string) => void
  onClearHistory: () => Promise<void>
}

export function CommandHistoryModal({
  isOpen,
  onClose,
  entries,
  allEntries,
  onSelectCommand,
  onClearHistory
}: CommandHistoryModalProps): React.JSX.Element {
  const { t } = useTranslation('shell')
  const { t: tCommon } = useTranslation('common')
  const [query, setQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(0)
  const [filterMode, setFilterMode] = useState<FilterMode>('this-project')
  const [showClearConfirm, setShowClearConfirm] = useState(false)
  const [isClearing, setIsClearing] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)
  const virtuosoRef = useRef<VirtuosoHandle>(null)

  // Get entries based on filter mode
  const baseEntries = useMemo(() => {
    return filterMode === 'this-project' ? entries : allEntries
  }, [filterMode, entries, allEntries])

  // Filter entries based on query
  const filteredEntries = useMemo(() => {
    if (!query) return baseEntries
    const lowerQuery = query.toLowerCase()
    return baseEntries.filter((e) => e.command.toLowerCase().includes(lowerQuery))
  }, [baseEntries, query])

  // Reset selection when query or filter changes
  useEffect(() => {
    setSelectedIndex(0)
  }, [])

  // Reset state when modal opens or closes
  useEffect(() => {
    if (isOpen) {
      setQuery('')
      setSelectedIndex(0)
      setFilterMode('this-project')
      // Blur terminal first so xterm doesn't hold focus, then focus the input
      if (document.activeElement instanceof HTMLElement) {
        document.activeElement.blur()
      }
      setTimeout(() => inputRef.current?.focus(), 0)
    }
    // Always reset confirmation state on any isOpen change
    setShowClearConfirm(false)
    setIsClearing(false)
  }, [isOpen])

  // Scroll selected item into view using Virtuoso
  useEffect(() => {
    if (virtuosoRef.current && filteredEntries.length > 0) {
      virtuosoRef.current.scrollToIndex(selectedIndex)
    }
  }, [selectedIndex, filteredEntries.length])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      } else if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSelectedIndex((i) => Math.min(i + 1, filteredEntries.length - 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSelectedIndex((i) => Math.max(i - 1, 0))
      } else if (e.key === 'Enter') {
        e.preventDefault()
        if (filteredEntries[selectedIndex]) {
          onSelectCommand(filteredEntries[selectedIndex].command)
          onClose()
        }
      }
    },
    [filteredEntries, selectedIndex, onSelectCommand, onClose]
  )

  const handleClearConfirm = useCallback(async () => {
    if (isClearing) return
    setIsClearing(true)
    try {
      await onClearHistory()
      setShowClearConfirm(false)
    } catch {
      // Keep dialog open on failure - parent already showed toast
    } finally {
      setIsClearing(false)
    }
  }, [onClearHistory, isClearing])

  const formatTime = (timestamp: number): string => {
    const date = new Date(timestamp)
    const now = new Date()
    const diffMs = now.getTime() - date.getTime()
    const diffMins = Math.floor(diffMs / 60000)
    const diffHours = Math.floor(diffMs / 3600000)
    const diffDays = Math.floor(diffMs / 86400000)

    if (diffMins < 1) return tCommon('time.justNow')
    if (diffMins < 60) return tCommon('time.minutesAgo', { count: diffMins })
    if (diffHours < 24) return tCommon('time.hoursAgo', { count: diffHours })
    if (diffDays < 7) return tCommon('time.daysAgo', { count: diffDays })
    return formatDate(date)
  }

  return (
    <AnimatePresence>
      {isOpen && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-[60] flex flex-col items-center bg-black/70 pt-[10vh] backdrop-blur-[2px]"
          onClick={onClose}
        >
          <motion.div
            initial={{ opacity: 0, y: -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -6 }}
            transition={{ duration: 0.15 }}
            className="w-full max-w-2xl overflow-hidden rounded-md border border-border/80 bg-card shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
            onClick={(e) => e.stopPropagation()}
          >
            {/* Header */}
            <div className="flex h-9 items-center justify-between gap-2 border-b border-border/70 px-3">
              <div className="flex items-center gap-2">
                <History size={14} className="text-muted-foreground" />
                <span className="text-xs font-medium tracking-[-0.01em]">
                  {t('commandHistory.title')}
                </span>
              </div>
              <Select
                value={filterMode}
                onValueChange={(value) => setFilterMode(value as FilterMode)}
              >
                <SelectTrigger className="h-7 w-[140px] text-2xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="this-project">{t('commandHistory.thisProject')}</SelectItem>
                  <SelectItem value="all-projects">{t('commandHistory.allProjects')}</SelectItem>
                </SelectContent>
              </Select>
            </div>

            {/* Search Input */}
            <div className="border-b border-border/70 px-3 py-2">
              <input
                ref={inputRef}
                type="text"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={handleKeyDown}
                placeholder={t('commandHistory.searchPlaceholder')}
                className="h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none transition-[border-color,background-color] placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:bg-secondary/50 focus-visible:ring-1 focus-visible:ring-ring/35"
              />
            </div>

            {/* Command List */}
            {filteredEntries.length === 0 ? (
              <div className="flex flex-col items-center justify-center gap-2 px-6 py-12 text-center">
                <div className="flex size-8 items-center justify-center rounded-md bg-secondary/50">
                  <History size={16} className="text-muted-foreground" />
                </div>
                <p className="text-sm font-medium text-foreground">
                  {baseEntries.length === 0
                    ? t('commandHistory.noHistory')
                    : t('commandHistory.noMatches')}
                </p>
              </div>
            ) : (
              <div className="max-h-[50vh]">
                <Virtuoso
                  ref={virtuosoRef}
                  style={{ height: '50vh' }}
                  data={filteredEntries}
                  itemContent={(index, entry) => (
                    <div
                      key={entry.id}
                      onClick={() => {
                        onSelectCommand(entry.command)
                        onClose()
                      }}
                      className={cn(
                        'relative cursor-pointer px-3 py-2 transition-colors before:absolute before:bottom-1.5 before:left-0 before:top-1.5 before:w-px before:rounded-full before:bg-primary before:opacity-0 before:transition-opacity',
                        index === selectedIndex
                          ? 'bg-secondary/80 shadow-[inset_0_1px_0_0_hsl(var(--foreground)/0.04)] before:opacity-100'
                          : 'hover:bg-secondary/45'
                      )}
                    >
                      <div className="flex items-start justify-between gap-3">
                        <code className="flex-1 break-all font-mono text-xs text-foreground">
                          {entry.command}
                        </code>
                      </div>
                      <div className="mt-1 flex items-center gap-3 text-2xs text-muted-foreground">
                        <span className="flex items-center gap-1">
                          <Terminal size={11} />
                          {entry.terminalName}
                        </span>
                        <span className="flex items-center gap-1">
                          <Clock size={11} />
                          {formatTime(entry.timestamp)}
                        </span>
                      </div>
                    </div>
                  )}
                />
              </div>
            )}

            {/* Footer */}
            <div className="flex items-center justify-between border-t border-border/70 bg-secondary/20 px-3 py-1.5 text-2xs text-muted-foreground">
              <div className="flex items-center gap-3">
                <span className="flex items-center gap-1">
                  <kbd className="rounded-sm border border-border/80 bg-secondary/50 px-1 py-px font-mono text-3xs text-foreground/80">
                    ↑↓
                  </kbd>
                  {t('commandHistory.navigate')}
                </span>
                <span className="flex items-center gap-1">
                  <kbd className="rounded-sm border border-border/80 bg-secondary/50 px-1 py-px font-mono text-3xs text-foreground/80">
                    ↵
                  </kbd>
                  {t('commandHistory.insert')}
                </span>
                <span className="flex items-center gap-1">
                  <kbd className="rounded-sm border border-border/80 bg-secondary/50 px-1 py-px font-mono text-3xs text-foreground/80">
                    Esc
                  </kbd>
                  {t('commandHistory.close')}
                </span>
              </div>
              <button
                type="button"
                onClick={() => setShowClearConfirm(true)}
                className="inline-flex h-6 items-center gap-1 rounded-md px-1.5 text-destructive transition-colors hover:bg-destructive/10 disabled:cursor-not-allowed disabled:opacity-45"
                disabled={entries.length === 0 || filterMode !== 'this-project' || isClearing}
                title={
                  filterMode === 'all-projects'
                    ? t('commandHistory.clearOtherProjectHint')
                    : undefined
                }
              >
                <Trash2 size={12} />
                <span>{isClearing ? t('commandHistory.clearing') : t('commandHistory.clear')}</span>
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}

      {/* Clear History Confirmation Dialog */}
      <ConfirmDialog
        isOpen={showClearConfirm}
        title={t('commandHistory.clearTitle')}
        message={t('commandHistory.clearMessage')}
        confirmLabel={tCommon('actions.clear')}
        cancelLabel={tCommon('actions.cancel')}
        variant="danger"
        isLoading={isClearing}
        onConfirm={handleClearConfirm}
        onCancel={() => setShowClearConfirm(false)}
      />
    </AnimatePresence>
  )
}
