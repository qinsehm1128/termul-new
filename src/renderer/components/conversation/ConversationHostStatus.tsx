import { AlertTriangle, CheckCircle2, LoaderCircle, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useConversationHostBootstrapStore } from '@/hooks/use-conversation-host-bootstrap'
import { cn } from '@/lib/utils'

const labelKeyByState = {
  ready: 'conversationHost.states.ready',
  migrating: 'conversationHost.states.migrating',
  hybrid: 'conversationHost.states.hybrid',
  recovery: 'conversationHost.states.recovery',
  error: 'conversationHost.states.error'
} as const

const iconByState = {
  ready: CheckCircle2,
  migrating: LoaderCircle,
  hybrid: RefreshCw,
  recovery: AlertTriangle,
  error: AlertTriangle
} as const

export function ConversationHostStatus(): React.JSX.Element {
  const { t } = useTranslation('common')
  const status = useConversationHostBootstrapStore((state) => state.status)
  const loading = useConversationHostBootstrapStore((state) => state.loading)
  const visibleState = loading || !status ? 'migrating' : status.state
  const Icon = iconByState[visibleState]
  const code = status?.code ?? 'CONVERSATION_HOST_MIGRATING'

  return (
    <section
      role="status"
      aria-live={visibleState === 'error' || visibleState === 'recovery' ? 'assertive' : 'polite'}
      aria-label={t('conversationHost.ariaLabel')}
      data-testid="conversation-host-status"
      data-state={visibleState}
      className={cn(
        'fixed left-1/2 top-2 z-[70] flex max-w-[calc(100vw-1rem)] -translate-x-1/2 items-center gap-2 rounded-md border bg-background/95 px-3 py-2 text-xs shadow-sm backdrop-blur transition-opacity duration-500 sm:top-3 sm:text-sm',
        // The ready confirmation is transient: fade out instead of pinning a
        // permanent banner over the workspace.
        visibleState === 'ready' && 'pointer-events-none opacity-0',
        visibleState === 'ready' && 'border-border text-muted-foreground',
        visibleState === 'migrating' && 'border-border text-foreground',
        visibleState === 'hybrid' && 'border-amber-500/40 text-foreground',
        visibleState === 'recovery' && 'border-destructive/40 text-destructive',
        visibleState === 'error' && 'border-destructive text-destructive'
      )}
    >
      <Icon
        aria-hidden="true"
        className={cn('size-4 shrink-0', visibleState === 'migrating' && 'animate-spin')}
      />
      <span className="truncate font-medium">{t(labelKeyByState[visibleState])}</span>
      <code className="max-w-[45vw] truncate font-mono text-[10px] opacity-80 sm:text-xs">
        {code}
      </code>
      {status && status.recoveryItemCount > 0 && (
        <span className="shrink-0">
          <span className="sr-only">
            {t('conversationHost.recoveryCount', { count: status.recoveryItemCount })}
          </span>
          <span aria-hidden="true">{status.recoveryItemCount}</span>
        </span>
      )}
    </section>
  )
}
