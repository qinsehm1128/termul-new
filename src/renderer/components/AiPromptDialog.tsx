/**
 * AI Prompt Dialog — template picker with one-click copy.
 *
 * Provides a dialog that:
 * - Lists AI prompt templates (Cursor, Aider, Claude Code)
 * - Shows per-template variable filling for worktree context
 * - One-click copy button
 * - Per-tool labeling ("Paste this into [Tool]")
 */

import { Bot, Check, Copy, MessageSquare, Terminal } from 'lucide-react'
import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  type AiPromptTemplate,
  BUILT_IN_TEMPLATES,
  buildTemplateVariables,
  interpolateTemplate
} from '@/lib/ai-prompt-templates'
import { cn } from '@/lib/utils'

interface AiPromptDialogProps {
  isOpen: boolean
  onClose: () => void
  /** Worktree context for filling template variables */
  context?: {
    sourceBranch: string
    targetBranch?: string
    conflictFiles?: string[]
    worktreePath: string
    projectName: string
  }
}

const TOOL_ICONS: Record<string, typeof Bot> = {
  Cursor: MessageSquare,
  Aider: Terminal,
  'Claude Code': Bot
}

export function AiPromptDialog({ isOpen, onClose, context }: AiPromptDialogProps) {
  const { t } = useTranslation('chat')
  const { t: tProjects } = useTranslation('projects')
  const [selectedTemplate, setSelectedTemplate] = useState<AiPromptTemplate>(BUILT_IN_TEMPLATES[0])
  const [copied, setCopied] = useState(false)

  const generatedPrompt = context
    ? interpolateTemplate(
        selectedTemplate.template,
        buildTemplateVariables({
          sourceBranch: context.sourceBranch,
          targetBranch: context.targetBranch,
          conflictFiles: context.conflictFiles,
          worktreePath: context.worktreePath,
          projectName: context.projectName
        })
      )
    : ''

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(generatedPrompt)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Fallback for older browsers
      const textarea = document.createElement('textarea')
      textarea.value = generatedPrompt
      document.body.appendChild(textarea)
      textarea.select()
      document.execCommand('copy')
      document.body.removeChild(textarea)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    }
  }, [generatedPrompt])

  if (!isOpen) return null

  const _Icon = TOOL_ICONS[selectedTemplate.toolName] ?? MessageSquare

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-[2px]"
      role="dialog"
      aria-modal="true"
      aria-labelledby="ai-prompt-dialog-title"
      onKeyDown={(e) => {
        if (e.key === 'Escape') onClose()
      }}
    >
      <div className="flex max-h-[80vh] w-[600px] max-w-[90vw] flex-col overflow-hidden rounded-md border border-border/80 bg-popover shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]">
        {/* Header */}
        <div className="flex h-9 items-center justify-between border-b border-border/70 px-3">
          <h2
            id="ai-prompt-dialog-title"
            className="text-xs font-semibold tracking-[-0.01em] text-foreground"
          >
            {t('aiPrompt.title')}
          </h2>
          <button
            type="button"
            onClick={onClose}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            aria-label={t('aiPrompt.close')}
          >
            ✕
          </button>
        </div>

        {/* Template selector */}
        <div className="border-b border-border/70 px-3 py-2">
          <span className="label-group text-muted-foreground">{t('aiPrompt.tool')}</span>
          <div className="flex gap-2 mt-1">
            {BUILT_IN_TEMPLATES.map((tpl) => {
              const TplIcon = TOOL_ICONS[tpl.toolName] ?? MessageSquare
              const templateName = tProjects(tpl.nameKey, { defaultValue: tpl.name })
              const templateDescription = tProjects(tpl.descriptionKey, {
                defaultValue: tpl.description
              })
              return (
                <button
                  type="button"
                  key={tpl.id}
                  onClick={() => setSelectedTemplate(tpl)}
                  aria-label={templateName}
                  title={templateDescription}
                  className={cn(
                    'inline-flex h-8 items-center gap-1.5 rounded-md px-3 text-xs transition-colors',
                    selectedTemplate.id === tpl.id
                      ? 'bg-primary text-primary-foreground'
                      : 'bg-secondary text-muted-foreground hover:text-foreground'
                  )}
                >
                  <TplIcon size={12} />
                  {tpl.toolName}
                </button>
              )
            })}
          </div>
        </div>

        {/* Prompt preview */}
        <div className="flex-1 overflow-auto px-4 py-3">
          <div className="flex items-center justify-between mb-2">
            <span className="label-group text-muted-foreground">
              {t('aiPrompt.pasteInto', {
                template: tProjects(selectedTemplate.nameKey, {
                  defaultValue: selectedTemplate.name
                }),
                tool: selectedTemplate.toolName
              })}
            </span>
          </div>
          <pre className="max-h-[300px] overflow-auto whitespace-pre-wrap rounded-md border border-border/80 bg-secondary/35 p-3 font-mono text-xs leading-relaxed text-foreground">
            {generatedPrompt || t('aiPrompt.missingContext')}
          </pre>
        </div>

        {/* Footer */}
        <div className="flex h-10 items-center justify-between border-t border-border/70 bg-secondary/20 px-4">
          <span className="text-3xs text-muted-foreground">
            {t('aiPrompt.variables', { variables: selectedTemplate.variables.join(', ') })}
          </span>
          <button
            type="button"
            onClick={handleCopy}
            disabled={!generatedPrompt}
            className={cn(
              'inline-flex h-8 items-center gap-1.5 rounded-md px-3 text-xs font-medium transition-colors focus:outline-none focus-visible:ring-1 focus-visible:ring-ring',
              copied
                ? 'bg-success/10 text-success'
                : 'bg-primary text-primary-foreground hover:bg-primary/90',
              !generatedPrompt && 'cursor-not-allowed opacity-50'
            )}
          >
            {copied ? (
              <>
                <Check size={12} /> {t('aiPrompt.copied')}
              </>
            ) : (
              <>
                <Copy size={12} /> {t('aiPrompt.copy')}
              </>
            )}
          </button>
        </div>
      </div>
    </div>
  )
}
