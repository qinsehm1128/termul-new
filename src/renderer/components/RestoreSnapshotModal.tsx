import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { AlertTriangle, RotateCcw, X } from 'lucide-react'
import { type KeyboardEvent, useCallback, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import type { Snapshot } from '@/types/project'

interface RestoreSnapshotModalProps {
  isOpen: boolean
  snapshot: Snapshot | null
  hasRunningProcesses: boolean
  onClose: () => void
  onRestore: () => Promise<void> | void
  isRestoring: boolean
}

export function RestoreSnapshotModal({
  isOpen,
  snapshot,
  hasRunningProcesses,
  onClose,
  onRestore,
  isRestoring
}: RestoreSnapshotModalProps): React.JSX.Element {
  const { t } = useTranslation('workspace')
  const reducedMotion = useReducedMotion() ?? false
  // Handle Escape key to close modal
  useEffect(() => {
    if (!isOpen) return

    const handleEscape = (e: globalThis.KeyboardEvent): void => {
      if (e.key === 'Escape' && !isRestoring) {
        e.preventDefault()
        onClose()
      }
    }

    window.addEventListener('keydown', handleEscape)
    return () => window.removeEventListener('keydown', handleEscape)
  }, [isOpen, isRestoring, onClose])

  const handleRestore = useCallback(async () => {
    if (!isRestoring) {
      await onRestore()
    }
  }, [isRestoring, onRestore])

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Enter' && !isRestoring) {
        e.preventDefault()
        handleRestore()
      } else if (e.key === 'Escape' && !isRestoring) {
        e.preventDefault()
        onClose()
      }
    },
    [isRestoring, handleRestore, onClose]
  )

  return (
    <AnimatePresence>
      {isOpen && snapshot && (
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
            className="w-[480px] overflow-hidden rounded-md border border-border/80 bg-card shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={handleKeyDown}
          >
            {/* Header */}
            <div className="flex h-9 items-center justify-between border-b border-border/70 px-3">
              <h3 className="flex items-center gap-2 text-xs font-semibold tracking-[-0.01em] text-foreground">
                <RotateCcw size={14} />
                {t('snapshots.restoreTitle')}
              </h3>
              <button
                onClick={onClose}
                disabled={isRestoring}
                className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
              >
                <X size={14} />
              </button>
            </div>

            {/* Content */}
            <div className="p-4 space-y-4">
              <p className="text-sm text-foreground">
                {t('snapshots.restoreConfirm', { name: snapshot.name })}
              </p>

              <p className="text-sm text-muted-foreground">
                {t('snapshots.restoreDescription', { count: snapshot.paneCount })}
              </p>

              {hasRunningProcesses && (
                <div className="flex items-start gap-2 rounded-md border border-warning/35 bg-warning/10 p-3">
                  <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warning" />
                  <div className="text-sm text-warning">
                    <span className="font-medium">{t('snapshots.warning')}</span>{' '}
                    {t('snapshots.runningWarning')}
                  </div>
                </div>
              )}
            </div>

            {/* Footer */}
            <div className="flex h-10 items-center justify-end gap-2 border-t border-border/70 bg-secondary/20 px-4">
              <button
                onClick={onClose}
                disabled={isRestoring}
                className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
              >
                {t('snapshots.cancel')}
              </button>
              <button
                onClick={handleRestore}
                disabled={isRestoring}
                className="inline-flex h-8 items-center gap-1.5 rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
              >
                <RotateCcw size={12} className={isRestoring ? 'animate-spin' : ''} />
                {isRestoring ? t('snapshots.restoring') : t('snapshots.restore')}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
