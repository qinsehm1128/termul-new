import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { TERMINAL_STATUS_DOT_CLASS } from '@/components/ui/motion-primitives'
import { AgentIcon } from '@/features/agent-session/agents/AgentIcon'
import { useConversationStore } from '@/features/agent-session/stores/conversation-store'
import { buildConversationTerminalNames } from '@/lib/conversation-terminal-names'
import { buildTerminalBoard, terminalBoardStatus } from '@/lib/terminal-board'
import { openBoardTerminal } from '@/lib/terminal-board-navigation'
import { cn } from '@/lib/utils'
import { useProjectStore } from '@/stores/project-store'
import { useTerminalStore } from '@/stores/terminal-store'

/**
 * Vertical terminal list docked at the right edge of the pane area.
 *
 * Deliberately not in the right rail: that rail already stacks the CLI panel,
 * the file tree and the SSH explorer, and a fourth tenant would just fight
 * them for width. Living inside the centre column keeps it independent.
 *
 * Unlike the switcher row this is never scoped — the row is bounded on
 * purpose, so a list that repeated the same subset would earn none of the
 * space it costs. Grouping is the board's, so both surfaces agree on where a
 * terminal belongs.
 */
export function TerminalListPanel(): React.JSX.Element {
  const { t } = useTranslation('terminal')
  const navigate = useNavigate()

  const terminals = useTerminalStore((state) => state.terminals)
  const activeTerminalId = useTerminalStore((state) => state.activeTerminalId)
  const projects = useProjectStore((state) => state.projects)
  const groups = useProjectStore((state) => state.groups)

  const conversationSummaries = useConversationStore((state) => state.summariesById)
  const conversationGroupName = t('switcher.conversations')
  const quickGroupName = t('board.quickTerminals')
  const sshGroupName = t('board.sshTerminals')
  const conversationNames = useMemo(
    () =>
      buildConversationTerminalNames(
        terminals,
        conversationSummaries,
        t('switcher.untitledConversation', { defaultValue: 'Untitled conversation' })
      ),
    [terminals, conversationSummaries, t]
  )
  const board = useMemo(
    () =>
      buildTerminalBoard(terminals, projects, groups, conversationNames, conversationGroupName, {
        quick: quickGroupName,
        ssh: sshGroupName
      }),
    [
      terminals,
      projects,
      groups,
      conversationNames,
      conversationGroupName,
      quickGroupName,
      sshGroupName
    ]
  )

  const open = (terminalId: string, projectId: string | undefined): void => {
    openBoardTerminal({ projectId: projectId ?? null, terminalId, navigate })
    useTerminalStore.getState().selectTerminal(terminalId)
  }

  return (
    <aside
      className="flex h-full w-48 shrink-0 flex-col overflow-y-auto border-l border-border/70 bg-sidebar px-1.5 py-1.5"
      aria-label={t('switcher.listPanelLabel')}
    >
      {terminals.length === 0 ? (
        <p className="px-2 py-1 text-2xs text-muted-foreground">{t('board.empty')}</p>
      ) : (
        board.map((group) =>
          group.projects.map((project) => (
            <div key={`${group.groupId ?? 'none'}:${project.projectId}`} className="mb-1.5">
              <p className="truncate px-2 py-1 font-mono text-3xs font-medium uppercase tracking-wider text-muted-foreground/80">
                {project.projectName}
              </p>
              <div className="space-y-0.5">
                {project.terminals.map((terminal) => (
                  <button
                    key={terminal.id}
                    type="button"
                    onClick={() => open(terminal.id, terminal.projectId)}
                    title={terminal.name}
                    aria-current={terminal.id === activeTerminalId ? 'true' : undefined}
                    className={cn(
                      'flex h-7 w-full items-center gap-1.5 rounded-md px-2 text-left text-2xs font-medium transition-colors duration-150',
                      terminal.id === activeTerminalId
                        ? 'bg-sidebar-accent text-foreground shadow-[inset_0_0_0_1px_hsl(var(--border)/0.8)]'
                        : 'text-muted-foreground hover:bg-sidebar-accent/55 hover:text-foreground'
                    )}
                  >
                    <span
                      className={cn(
                        'size-1.5 shrink-0 rounded-full',
                        TERMINAL_STATUS_DOT_CLASS[terminalBoardStatus(terminal)]
                      )}
                      aria-hidden="true"
                    />
                    {terminal.agentId ? (
                      <AgentIcon
                        agentId={terminal.agentId}
                        name={terminal.agentName}
                        className="size-3"
                      />
                    ) : null}
                    <span className="truncate">{terminal.name}</span>
                  </button>
                ))}
              </div>
            </div>
          ))
        )
      )}
    </aside>
  )
}
