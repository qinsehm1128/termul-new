import { EyeOff } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { AgentIcon } from '@/components/agents/AgentIcon'
import { ListRow, ListRowMeta, pathBasename } from '@/components/lists'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import type { Terminal } from '@/types/project'

interface HiddenTerminalPopoverProps {
  terminals: Terminal[]
  onReopen: (terminalId: string) => void
  onStop: (terminalId: string) => void
}

export function HiddenTerminalPopover({
  terminals,
  onReopen,
  onStop
}: HiddenTerminalPopoverProps): React.JSX.Element | null {
  const { t } = useTranslation('terminal')
  if (terminals.length === 0) return null

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="inline-flex h-7 items-center gap-1.5 rounded-md px-2 text-2xs text-muted-foreground transition-colors duration-150 hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          aria-label={t('lifecycle.hiddenRunning')}
        >
          <EyeOff className="size-3.5" aria-hidden="true" />
          {t('lifecycle.hiddenCount', { count: terminals.length })}
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80 p-1.5">
        <p className="px-2 py-1.5 text-2xs text-muted-foreground">{t('lifecycle.hiddenRunning')}</p>
        {terminals.map((terminal) => (
          <ListRow
            key={terminal.id}
            density="compact"
            title={
              <span className="flex min-w-0 items-center gap-1.5">
                {terminal.agentId ? (
                  <AgentIcon
                    agentId={terminal.agentId}
                    name={terminal.agentName}
                    className="size-3.5"
                  />
                ) : null}
                <span className="truncate">{terminal.name}</span>
              </span>
            }
            titleAttr={terminal.name}
            preview={pathBasename(terminal.cwd) || terminal.cwd}
            meta={
              <ListRowMeta
                items={[
                  terminal.agentName ?? terminal.shell,
                  terminal.healthStatus === 'running' ? t('lifecycle.hiddenRunning') : null
                ]}
              />
            }
            trailing={
              <>
                <button
                  type="button"
                  className="h-9 rounded-md px-2 text-2xs hover:bg-accent"
                  onClick={() => onReopen(terminal.id)}
                >
                  {t('lifecycle.reopenNamed', { name: terminal.name })}
                </button>
                <button
                  type="button"
                  className="h-9 rounded-md px-2 text-2xs text-destructive hover:bg-destructive/10"
                  onClick={() => onStop(terminal.id)}
                >
                  {t('lifecycle.stopHidden')}
                </button>
              </>
            }
            onClick={() => onReopen(terminal.id)}
          />
        ))}
      </PopoverContent>
    </Popover>
  )
}
