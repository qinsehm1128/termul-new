import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { X } from 'lucide-react'
import { type KeyboardEvent, useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

interface CreateSnapshotModalProps {
  isOpen: boolean
  onClose: () => void
  onCreateSnapshot: (name: string, description?: string) => Promise<void> | void
}

export function CreateSnapshotModal({
  isOpen,
  onClose,
  onCreateSnapshot
}: CreateSnapshotModalProps): React.JSX.Element {
  const { t } = useTranslation('workspace')
  const reducedMotion = useReducedMotion() ?? false
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [isCreating, setIsCreating] = useState(false)

  // Reset form when modal opens
  useEffect(() => {
    if (isOpen) {
      setName('')
      setDescription('')
      setIsCreating(false)
    }
  }, [isOpen])

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

  const handleCreate = useCallback(async () => {
    if (name.trim() && !isCreating) {
      setIsCreating(true)
      try {
        await onCreateSnapshot(name.trim(), description.trim() || undefined)
        onClose()
      } catch (error) {
        console.error('Failed to create snapshot:', error)
        // Keep modal open on error so user can retry
      } finally {
        setIsCreating(false)
      }
    }
  }, [name, description, isCreating, onCreateSnapshot, onClose])

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Enter' && name.trim() && !isCreating) {
        e.preventDefault()
        handleCreate()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      }
    },
    [name, isCreating, handleCreate, onClose]
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
            className="w-[480px] overflow-hidden rounded-md border border-border/80 bg-card shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={handleKeyDown}
          >
            {/* Header */}
            <div className="flex h-9 items-center justify-between border-b border-border/70 px-3">
              <h3 className="text-xs font-semibold tracking-[-0.01em] text-foreground">
                {t('snapshots.createTitle')}
              </h3>
              <button
                onClick={onClose}
                className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X size={14} />
              </button>
            </div>

            {/* Content */}
            <div className="space-y-3 p-4">
              <div className="space-y-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('snapshots.name')}
                </label>
                <input
                  type="text"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={t('snapshots.namePlaceholder')}
                  className="h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                />
              </div>

              <div className="space-y-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('snapshots.descriptionOptional')}
                </label>
                <textarea
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder={t('snapshots.descriptionPlaceholder')}
                  rows={3}
                  className="w-full resize-none rounded-md border border-input/80 bg-secondary/35 px-2.5 py-2 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                />
              </div>
            </div>

            {/* Footer */}
            <div className="flex h-10 items-center justify-end gap-2 border-t border-border/70 bg-secondary/20 px-4">
              <button
                onClick={onClose}
                className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('snapshots.cancel')}
              </button>
              <button
                onClick={handleCreate}
                disabled={!name.trim() || isCreating}
                className="inline-flex h-8 items-center rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
              >
                {isCreating ? t('snapshots.creating') : t('snapshots.create')}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
