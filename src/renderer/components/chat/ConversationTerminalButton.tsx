import { Loader2, SquareTerminal, X } from 'lucide-react'
import { useCallback, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { logFrontendError } from '@/lib/log-api'
import { spawnTerminalInPane } from '@/lib/terminal-spawn'
import { cn } from '@/lib/utils'
import { getDefaultCwdForProject } from '@/lib/worktree-context'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { useConversationStore } from '@/stores/conversation-store'
import { useProjectStore } from '@/stores/project-store'
import { useConversationTerminals, useTerminalStore } from '@/stores/terminal-store'
import { useWorkspaceStore } from '@/stores/workspace-store'

/**
 * Open a shell that belongs to this conversation, from the composer.
 *
 * The affordance next to it launches an *agent*; this one launches a plain
 * terminal in the same scope, for the times the user wants to run the command
 * themselves rather than ask for it.
 *
 * # Why closing here terminates, and the tab's × does not
 *
 * `WorkspaceLayout.handleCloseTerminal` deliberately treats the two kinds
 * differently: closing a project terminal's tab kills its PTY, while closing a
 * *conversation* terminal's tab only retires the view and leaves the process
 * running, because the conversation may still be using it and the tab is not
 * the owner.
 *
 * That is also exactly the "it just stays in the background" complaint. So this
 * button owns the other half: its × terminates. One control creates the
 * terminal and one control ends it, and nothing ends it behind the user's back
 * — a shell running a dev server survives closing the conversation, switching
 * away, and closing its tab. Only this × stops it.
 */
export function ConversationTerminalButton(): React.JSX.Element | null {
  const { t } = useTranslation('chat')
  const [open, setOpen] = useState(false)
  const [pending, setPending] = useState(false)

  const conversationId = useConversationStore((state) => state.activeConversationId)
  const workspaceCwd = useConversationStore((state) =>
    conversationId ? state.summariesById[conversationId]?.workspaceCwd : undefined
  )
  const terminals = useConversationTerminals(conversationId)

  const spawn = useCallback(async (): Promise<void> => {
    if (!conversationId) return
    const paneId = useWorkspaceStore.getState().activePaneId
    if (!paneId) return

    // The same two lines `WorkspaceLayout.handleCreateTerminalInPane` uses for
    // its Conversation branch. Resolved here rather than reached for through a
    // prop chain, but deliberately the *same* rule: a conversation runs in its
    // own workspace directory, and only falls back to the active project's when
    // it has none.
    const projectId = useProjectStore.getState().activeProjectId
    const cwd = workspaceCwd ?? getDefaultCwdForProject(projectId)
    if (!cwd) {
      void logFrontendError({
        level: 'warn',
        source: 'conversation-terminal.spawn',
        message: `No workspace directory resolvable for conversation ${conversationId}`
      })
      return
    }

    setPending(true)
    try {
      const result = await spawnTerminalInPane(paneId, projectId, cwd, {
        conversationId,
        maxTerminalsPerProject: useAppSettingsStore.getState().settings.maxTerminalsPerProject
      })
      if (!result.success) {
        void logFrontendError({
          level: 'error',
          source: 'conversation-terminal.spawn',
          message: result.error ?? 'unknown error'
        })
      }
    } finally {
      setPending(false)
    }
  }, [conversationId, workspaceCwd])

  const terminate = useCallback(async (terminalId: string): Promise<void> => {
    setPending(true)
    try {
      await useTerminalStore.getState().terminateTerminalResource(terminalId)
    } finally {
      setPending(false)
    }
  }, [])

  // Nothing to scope a terminal to outside an open conversation.
  if (!conversationId) return null

  const count = terminals.length

  if (count === 0) {
    return (
      <button
        type="button"
        data-testid="conversation-terminal-start"
        onClick={() => void spawn()}
        disabled={pending}
        title={t('composer.conversationTerminal.start')}
        aria-label={t('composer.conversationTerminal.start')}
        className="inline-flex h-7 items-center gap-1.5 rounded-md border border-border/70 bg-secondary/35 px-2 text-xs text-muted-foreground transition-colors hover:border-border hover:bg-secondary/60 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
      >
        {pending ? (
          <Loader2 size={13} className="animate-spin" />
        ) : (
          <SquareTerminal size={13} aria-hidden="true" />
        )}
        {t('composer.conversationTerminal.start')}
      </button>
    )
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          data-testid="conversation-terminal-badge"
          aria-label={t('composer.conversationTerminal.manageAria', { count })}
          className={cn(
            'inline-flex h-7 items-center gap-1.5 rounded-md border border-border/70 bg-secondary/35 px-2 text-xs transition-colors hover:border-border hover:bg-secondary/60',
            'text-foreground'
          )}
        >
          <SquareTerminal size={13} aria-hidden="true" />
          <span className="tabular-nums">{count}</span>
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 p-2">
        <p className="px-1 pb-1.5 text-xs text-muted-foreground">
          {t('composer.conversationTerminal.residentHint')}
        </p>
        <ul className="space-y-0.5">
          {terminals.map((terminal) => (
            <li
              key={terminal.id}
              data-testid="conversation-terminal-row"
              data-terminal-id={terminal.id}
              className="flex items-center gap-2 rounded px-1 py-1 text-xs hover:bg-secondary/50"
            >
              <SquareTerminal size={12} className="shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate">{terminal.name}</span>
              <button
                type="button"
                onClick={() => void terminate(terminal.id)}
                disabled={pending}
                title={t('composer.conversationTerminal.close')}
                aria-label={t('composer.conversationTerminal.closeNamed', {
                  name: terminal.name
                })}
                className="shrink-0 rounded p-0.5 text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive disabled:cursor-not-allowed disabled:opacity-50"
              >
                <X size={12} />
              </button>
            </li>
          ))}
        </ul>
        <button
          type="button"
          data-testid="conversation-terminal-add"
          onClick={() => void spawn()}
          disabled={pending}
          className="mt-1 inline-flex h-7 w-full items-center justify-center gap-1.5 rounded-md bg-secondary/50 text-xs text-foreground transition-colors hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-50"
        >
          {pending && <Loader2 size={12} className="animate-spin" />}
          {t('composer.conversationTerminal.start')}
        </button>
      </PopoverContent>
    </Popover>
  )
}
