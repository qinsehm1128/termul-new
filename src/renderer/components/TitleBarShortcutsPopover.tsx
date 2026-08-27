import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { Keyboard } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { ShortcutRecorder } from '@/components/ShortcutRecorder'
import { useResetShortcut, useUpdateShortcut } from '@/hooks/use-keyboard-shortcuts'
import { useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
import type { KeyboardShortcut } from '@/types/settings'

const QUICK_SHORTCUT_IDS = [
  'commandPalette',
  'commandHistory',
  'newTerminal',
  'newBrowserTab',
  'toggleFileExplorer',
  'toggleCliSessionPanel',
  'sidebarToggle',
  'zoomIn',
  'zoomOut',
  'zoomReset'
] as const

interface TitleBarShortcutsPopoverProps {
  buttonClassName: string
  open?: boolean
  onOpenChange?: (open: boolean) => void
}

export function TitleBarShortcutsPopover({
  buttonClassName,
  open,
  onOpenChange
}: TitleBarShortcutsPopoverProps): React.JSX.Element {
  const { t } = useTranslation('shell')
  const reducedMotion = useReducedMotion() ?? false
  const [openFallback, setOpenFallback] = useState(false)
  const isOpen = open ?? openFallback
  const shortcuts = useKeyboardShortcutsStore((state) => state.shortcuts)
  const updateShortcut = useUpdateShortcut()
  const resetShortcut = useResetShortcut()
  const closeButtonRef = useRef<HTMLButtonElement>(null)

  const quickShortcuts = QUICK_SHORTCUT_IDS.map((id) => shortcuts[id]).filter(
    (shortcut): shortcut is KeyboardShortcut => Boolean(shortcut)
  )

  const setOpen = useCallback(
    (nextOpen: boolean) => {
      if (onOpenChange) {
        onOpenChange(nextOpen)
        return
      }

      setOpenFallback(nextOpen)
    },
    [onOpenChange]
  )

  const handleUpdate = (id: string, customKey: string): void => {
    void updateShortcut(id, customKey).catch((error: unknown) => {
      toast.error(error instanceof Error ? error.message : t('shortcuts.saveFailed'))
    })
  }

  const handleReset = (id: string): void => {
    void resetShortcut(id).catch((error: unknown) => {
      toast.error(error instanceof Error ? error.message : t('shortcuts.resetFailed'))
    })
  }

  useEffect(() => {
    if (!isOpen) return

    if (document.activeElement instanceof HTMLElement) {
      document.activeElement.blur()
    }
    setTimeout(() => closeButtonRef.current?.focus(), 0)
  }, [isOpen])

  useEffect(() => {
    if (!isOpen) return

    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return

      const activeElement = document.activeElement
      if (
        activeElement instanceof HTMLElement &&
        activeElement.closest('[data-shortcut-recorder="true"]')
      ) {
        return
      }

      event.preventDefault()
      event.stopPropagation()
      setOpen(false)
    }

    window.addEventListener('keydown', handleKeyDown, { capture: true })
    return () => window.removeEventListener('keydown', handleKeyDown, { capture: true })
  }, [isOpen, setOpen])

  return (
    <>
      <button
        type="button"
        className={buttonClassName}
        title={t('titleBar.keyboardShortcuts')}
        aria-label={t('titleBar.openKeyboardShortcuts')}
        aria-expanded={isOpen}
        onClick={(event) => {
          event.stopPropagation()
          setOpen(!isOpen)
        }}
      >
        <Keyboard size={16} className={isOpen ? 'text-foreground' : 'text-muted-foreground'} />
      </button>

      <AnimatePresence>
        {isOpen && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-[110] flex flex-col items-center bg-black/70 pt-[7vh] backdrop-blur-[2px]"
            onClick={() => setOpen(false)}
          >
            <motion.div
              role="dialog"
              aria-modal="true"
              aria-labelledby="shortcut-menu-title"
              initial={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
              transition={{ duration: reducedMotion ? 0 : 0.15 }}
              className="w-full max-w-lg overflow-hidden rounded-md border border-border/80 bg-popover shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
              onClick={(event) => event.stopPropagation()}
            >
              <div className="flex items-start justify-between gap-3 border-b border-border/70 px-3 py-2">
                <div>
                  <div
                    id="shortcut-menu-title"
                    className="flex items-center gap-2 text-xs font-semibold tracking-[-0.01em] text-foreground"
                  >
                    <Keyboard aria-hidden="true" size={14} />
                    {t('titleBar.shortcutMenu')}
                  </div>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {t('titleBar.shortcutMenuDescription')}
                  </p>
                </div>
                <button
                  ref={closeButtonRef}
                  type="button"
                  onClick={() => setOpen(false)}
                  className="inline-flex h-7 items-center rounded-md px-2 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  Esc
                </button>
              </div>

              <div className="max-h-[52vh] overflow-y-auto px-2 py-2">
                {quickShortcuts.length === 0 ? (
                  <div className="px-2 py-6 text-center text-xs text-muted-foreground">
                    {t('titleBar.noShortcuts')}
                  </div>
                ) : (
                  <div className="space-y-1">
                    {quickShortcuts.map((shortcut) => (
                      <ShortcutRecorder
                        key={shortcut.id}
                        shortcut={shortcut}
                        allShortcuts={shortcuts}
                        onUpdate={handleUpdate}
                        onReset={handleReset}
                        variant="compact"
                      />
                    ))}
                  </div>
                )}
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  )
}
