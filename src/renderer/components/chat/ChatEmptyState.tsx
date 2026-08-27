import { motion, type Transition, useReducedMotion } from 'framer-motion'
import { Bug, FileText, ListChecks, Sparkles } from 'lucide-react'
import { useRuntimeTranslation } from '@/i18n/use-runtime-translation'
import type { AgentId } from '@/lib/acp-api'
import { cn } from '@/lib/utils'
import { useAgentIdentity } from '@/stores/acp-store'
import { AgentGlyph } from './AgentGlyph'
import { CHAT_CONTENT_WIDTH, CHAT_GUTTER_X } from './chat-layout'
import { CHAT_SPRING } from './chat-motion'

interface Suggestion {
  icon: React.ComponentType<{ className?: string }>
  key: 'explain' | 'bug' | 'tests' | 'changes'
  label: string
  prompt: string
}

const SUGGESTIONS: Suggestion[] = [
  {
    icon: Sparkles,
    key: 'explain',
    label: 'Explain this project',
    prompt: 'Give me a high-level overview of this codebase and how it is structured.'
  },
  {
    icon: Bug,
    key: 'bug',
    label: 'Find a bug',
    prompt: 'Look for potential bugs or edge cases in the code I currently have open.'
  },
  {
    icon: ListChecks,
    key: 'tests',
    label: 'Write tests',
    prompt: 'Write unit tests for the file I am currently working on.'
  },
  {
    icon: FileText,
    key: 'changes',
    label: 'Summarize changes',
    prompt: 'Summarize my recent uncommitted git changes.'
  }
]

interface ChatEmptyStateProps {
  agentId: AgentId
  /** Seed the composer with a chosen prompt. */
  onPick?: (text: string) => void
}

/** First-run state for an empty thread: agent identity + clickable starter prompts. */
export function ChatEmptyState({ agentId, onPick }: ChatEmptyStateProps): React.JSX.Element {
  const t = useRuntimeTranslation('chat')
  const reduced = useReducedMotion() ?? false
  const { name, templateId } = useAgentIdentity(agentId)
  const transition = (i: number): Transition =>
    reduced ? { duration: 0 } : { ...CHAT_SPRING, delay: 0.04 * i }

  return (
    <div className={cn(CHAT_GUTTER_X, 'flex min-h-0 flex-1 flex-col items-center justify-center')}>
      <div
        className={cn(
          CHAT_CONTENT_WIDTH,
          'flex flex-col items-center justify-center gap-4 text-center'
        )}
      >
        <motion.div
          className="flex flex-col items-center gap-2.5"
          initial={reduced ? false : { opacity: 0, y: 8, scale: 0.96 }}
          animate={reduced ? { opacity: 1 } : { opacity: 1, y: 0, scale: 1 }}
          transition={transition(0)}
        >
          <div className="flex size-8 items-center justify-center">
            <AgentGlyph templateId={templateId} size={20} className="text-foreground/85" />
          </div>
          <div>
            <h2 className="text-sm font-medium text-foreground">
              {name
                ? t('empty.chatWith', 'Chat with {{name}}', { name })
                : t('empty.start', 'Start a conversation')}
            </h2>
            <p className="mt-1 text-xs text-muted-foreground">
              {t('empty.hint', 'Ask anything, or try one of these:')}
            </p>
          </div>
        </motion.div>

        {onPick && (
          <div className="w-full max-w-sm overflow-hidden rounded-md border border-border/80 bg-secondary/30 shadow-[inset_0_1px_0_0_hsl(var(--foreground)/0.04)] ring-1 ring-inset ring-background/70">
            {SUGGESTIONS.map((s, i) => (
              <motion.button
                key={s.key}
                type="button"
                onClick={() => onPick(t(`empty.suggestions.${s.key}.prompt`, s.prompt))}
                initial={reduced ? false : { opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                transition={transition(i + 1)}
                className="flex w-full items-center gap-2.5 border-b border-border/50 px-2.5 py-2 text-left text-sm text-muted-foreground transition-colors last:border-b-0 hover:bg-secondary/55 hover:text-foreground"
              >
                <s.icon className="size-3.5 shrink-0 text-muted-foreground" />
                <span className="truncate">{t(`empty.suggestions.${s.key}.label`, s.label)}</span>
              </motion.button>
            ))}
          </div>
        )}

        <p className="text-2xs text-muted-foreground">
          {t('empty.type', 'Type')}{' '}
          <kbd className="rounded-sm border border-border/80 bg-secondary/50 px-1 py-px font-mono text-3xs text-foreground/80">
            /
          </kbd>{' '}
          {t('empty.commandsHint', 'for commands & skills')}
        </p>
      </div>
    </div>
  )
}
