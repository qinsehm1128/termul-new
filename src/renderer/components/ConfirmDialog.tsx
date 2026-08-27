import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { AlertTriangle } from 'lucide-react'
import { type KeyboardEvent, useCallback, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { cn } from '@/lib/utils'

interface ConfirmDialogProps {
  isOpen: boolean
  title: string
  message: string
  children?: React.ReactNode
  confirmLabel?: string
  cancelLabel?: string
  secondaryAction?: {
    label: string
    onClick: () => void
  }
  variant?: 'default' | 'danger'
  isLoading?: boolean
  onConfirm: () => void
  onCancel: () => void
}

export function ConfirmDialog({
  isOpen,
  title,
  message,
  children,
  confirmLabel,
  cancelLabel,
  secondaryAction,
  variant = 'default',
  isLoading = false,
  onConfirm,
  onCancel
}: ConfirmDialogProps): React.JSX.Element {
  const { t } = useTranslation('common')
  const reducedMotion = useReducedMotion() ?? false

  // Handle Escape key to close dialog
  useEffect(() => {
    if (!isOpen) return

    const handleEscape = (e: globalThis.KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onCancel()
      }
    }

    window.addEventListener('keydown', handleEscape)
    return () => window.removeEventListener('keydown', handleEscape)
  }, [isOpen, onCancel])

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Enter') {
        e.preventDefault()
        onConfirm()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        onCancel()
      }
    },
    [onConfirm, onCancel]
  )

  return (
    <AnimatePresence>
      {isOpen && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-[2px]"
          onClick={onCancel}
        >
          <motion.div
            initial={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
            transition={{ duration: reducedMotion ? 0 : 0.15 }}
            className="w-[400px] overflow-hidden rounded-md border border-border/80 bg-card shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={handleKeyDown}
            tabIndex={-1}
          >
            {/* Content */}
            <div className="p-4">
              <div className="flex items-start gap-3">
                {variant === 'danger' && (
                  <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-destructive/10">
                    <AlertTriangle className="size-4 text-destructive" />
                  </div>
                )}
                <div className="flex-1">
                  <h3 className="mb-1 text-xs font-semibold tracking-[-0.01em] text-foreground">
                    {title}
                  </h3>
                  <p className="text-sm text-muted-foreground">{message}</p>
                  {children && <div className="mt-3">{children}</div>}
                </div>
              </div>
            </div>

            {/* Footer */}
            <div className="flex h-10 items-center justify-end gap-2 border-t border-border/70 bg-secondary/20 px-4">
              <button
                onClick={onCancel}
                disabled={isLoading}
                className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
              >
                {cancelLabel ?? t('actions.cancel')}
              </button>
              {secondaryAction && (
                <button
                  onClick={secondaryAction.onClick}
                  disabled={isLoading}
                  className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-destructive transition-colors hover:bg-destructive/10 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {secondaryAction.label}
                </button>
              )}
              <button
                onClick={onConfirm}
                disabled={isLoading}
                className={cn(
                  'inline-flex h-8 items-center rounded-md px-3 text-xs font-medium transition-colors focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50',
                  variant === 'danger'
                    ? 'bg-destructive text-destructive-foreground hover:bg-destructive/90'
                    : 'bg-primary text-primary-foreground hover:bg-primary/90'
                )}
              >
                {isLoading ? t('dialog.loading') : (confirmLabel ?? t('actions.confirm'))}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
