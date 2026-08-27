import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { AlertTriangle, Download, X } from 'lucide-react'
import { type KeyboardEvent, useCallback, useEffect } from 'react'
import { useTranslation } from 'react-i18next'

interface UpdateReadyModalProps {
  isOpen: boolean
  version: string
  releaseNotes?: string
  hasActiveTerminals: boolean
  onRestartNow: () => void
  onSkip: () => void
  onClose: () => void
}

export function UpdateReadyModal({
  isOpen,
  version,
  releaseNotes,
  hasActiveTerminals,
  onRestartNow,
  onSkip,
  onClose
}: UpdateReadyModalProps): React.JSX.Element {
  const { t } = useTranslation('shell')
  const reducedMotion = useReducedMotion() ?? false

  // Handle Escape key to close modal
  useEffect(() => {
    if (!isOpen) return

    const handleEscape = (e: globalThis.KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      }
    }

    window.addEventListener('keydown', handleEscape)
    return () => window.removeEventListener('keydown', handleEscape)
  }, [isOpen, onClose])

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      }
    },
    [onClose]
  )

  return (
    <AnimatePresence>
      {isOpen && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-[2px]"
          onClick={onClose}
        >
          <motion.div
            initial={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
            transition={{ duration: reducedMotion ? 0 : 0.15 }}
            className="w-[500px] overflow-hidden rounded-md border border-border/80 bg-card shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={handleKeyDown}
            tabIndex={-1}
          >
            {/* Header */}
            <div className="flex h-9 items-center justify-between border-b border-border/70 px-3">
              <div className="flex items-center gap-2">
                <div className="flex size-5 items-center justify-center rounded-md bg-success/10">
                  <Download className="size-3 text-success" />
                </div>
                <h3 className="text-xs font-semibold tracking-[-0.01em] text-foreground">
                  {t('updateReadyModal.title')}
                </h3>
              </div>
              <button
                onClick={onClose}
                className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                aria-label={t('updateReadyModal.close')}
              >
                <X size={14} />
              </button>
            </div>

            {/* Content */}
            <div className="p-4 space-y-4">
              {/* Version Info */}
              <div>
                <div className="flex items-baseline gap-2 mb-2">
                  <span className="text-xs font-medium text-muted-foreground">
                    {t('updateReadyModal.version')}
                  </span>
                  <span className="text-sm font-semibold text-foreground">{version}</span>
                </div>
              </div>

              {/* Release Notes */}
              {releaseNotes && (
                <div>
                  <label className="block text-xs font-medium text-muted-foreground mb-1.5">
                    {t('updateReadyModal.releaseNotes')}
                  </label>
                  <div className="max-h-[200px] overflow-y-auto rounded-md border border-border/80 bg-secondary/35 px-3 py-2 text-sm text-foreground">
                    <div className="whitespace-pre-wrap text-xs leading-relaxed">
                      {releaseNotes}
                    </div>
                  </div>
                </div>
              )}

              {/* Warning about running terminals */}
              {hasActiveTerminals && (
                <div className="flex items-start gap-2 rounded-md border border-warning/35 bg-warning/10 px-3 py-2">
                  <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warning" />
                  <div className="flex-1">
                    <p className="text-xs text-warning">{t('updateReadyModal.warning')}</p>
                  </div>
                </div>
              )}
            </div>

            {/* Footer */}
            <div className="flex h-10 flex-wrap items-center justify-end gap-2 border-t border-border/70 bg-secondary/20 px-4">
              <button
                onClick={onSkip}
                className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('updateReadyModal.skip')}
              </button>
              <button
                onClick={onRestartNow}
                className="inline-flex h-8 items-center rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('updateReadyModal.restart')}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
