import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { ExternalLink, Sparkles, X } from 'lucide-react'
import { type KeyboardEvent, useCallback, useEffect, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { renderChatMarkdown } from '@/lib/chat-markdown'
import { openerApi } from '@/lib/tauri-opener-api'

interface WhatsNewModalProps {
  isOpen: boolean
  version: string
  notes?: string | null
  htmlUrl?: string | null
  onClose: () => void
}

export function WhatsNewModal({
  isOpen,
  version,
  notes,
  htmlUrl,
  onClose
}: WhatsNewModalProps): React.JSX.Element {
  const { t } = useTranslation('shell')
  const reducedMotion = useReducedMotion() ?? false

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

  const handleViewOnGitHub = useCallback(() => {
    // Only hand http(s) URLs to the system opener; htmlUrl originates from a
    // network response, so guard against unexpected schemes.
    if (htmlUrl && /^https?:\/\//i.test(htmlUrl)) {
      void openerApi.openUrlWithSystemBrowser(htmlUrl)
    }
  }, [htmlUrl])

  // Release notes are GitHub-flavored markdown; render to sanitized HTML
  // (DOMPurify via the shared chat-markdown renderer) so headings, lists, and
  // links display properly instead of as raw markup.
  const notesHtml = useMemo(() => (notes ? renderChatMarkdown(notes) : null), [notes])

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
            role="dialog"
            aria-modal="true"
            aria-labelledby="whats-new-title"
            tabIndex={-1}
          >
            {/* Header */}
            <div className="flex h-9 items-center justify-between border-b border-border/70 px-3">
              <div className="flex items-center gap-2">
                <div className="flex size-5 items-center justify-center rounded-md bg-primary/10">
                  <Sparkles className="size-3 text-primary" />
                </div>
                <h3
                  id="whats-new-title"
                  className="text-xs font-semibold tracking-[-0.01em] text-foreground"
                >
                  {t('whatsNew.title')}
                </h3>
              </div>
              <button
                type="button"
                onClick={onClose}
                className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                aria-label={t('whatsNew.close')}
              >
                <X size={14} />
              </button>
            </div>

            {/* Content */}
            <div className="p-4 space-y-4">
              {/* Version Info */}
              <div className="flex items-baseline gap-2">
                <span className="text-xs font-medium text-muted-foreground">
                  {t('whatsNew.updatedTo')}
                </span>
                <span className="text-sm font-semibold text-foreground">{version}</span>
              </div>

              {/* Release Notes */}
              <div>
                <span className="block text-xs font-medium text-muted-foreground mb-1.5">
                  {t('whatsNew.releaseNotes')}
                </span>
                <div className="max-h-[320px] overflow-y-auto pr-1">
                  {notesHtml ? (
                    <div
                      className="chat-prose text-xs leading-relaxed text-foreground"
                      // biome-ignore lint/security/noDangerouslySetInnerHtml: HTML is sanitized via renderChatMarkdown (DOMPurify)
                      dangerouslySetInnerHTML={{ __html: notesHtml }}
                    />
                  ) : (
                    <p className="text-xs text-muted-foreground leading-relaxed">
                      {t('whatsNew.unavailable')}
                    </p>
                  )}
                </div>
              </div>
            </div>

            {/* Footer */}
            <div className="flex h-10 flex-wrap items-center justify-end gap-2 border-t border-border/70 bg-secondary/20 px-4">
              {htmlUrl && (
                <button
                  type="button"
                  onClick={handleViewOnGitHub}
                  className="inline-flex h-8 items-center gap-1.5 rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  <ExternalLink size={13} />
                  <span>{t('whatsNew.viewGithub')}</span>
                </button>
              )}
              <button
                type="button"
                onClick={onClose}
                className="inline-flex h-8 items-center rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('whatsNew.gotIt')}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
