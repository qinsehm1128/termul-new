import type { SSHProfile } from '@shared/types/ssh.types'
import { Terminal, WifiOff } from 'lucide-react'
import { ConnectedTerminal } from '@/components/terminal/ConnectedTerminal'
import type { useSSHConnection } from '@/hooks/use-ssh-connection'
import { useSshTranslation } from '@/hooks/use-ssh-translation'
import { useSSHEditorFile } from '@/stores/ssh-store'
import { SSHFileEditor } from './SSHFileEditor'

interface SSHWorkspaceProps {
  profile: SSHProfile
  conn: ReturnType<typeof useSSHConnection>
}

export function SSHWorkspace({ profile, conn }: SSHWorkspaceProps): React.JSX.Element {
  const t = useSshTranslation()
  const editingFile = useSSHEditorFile()

  return (
    <div className="flex h-full w-full overflow-hidden bg-background">
      {/* Right: Terminal + Editor area */}
      <div className="flex min-w-0 flex-1 flex-col">
        {/* Top bar */}
        <div className="flex h-8 items-center justify-between border-b border-border/70 bg-sidebar px-2.5 shadow-[inset_0_1px_0_hsl(var(--foreground)/0.025)]">
          <div className="flex items-center gap-2">
            <Terminal className="h-3.5 w-3.5 text-muted-foreground" />
            <span className="text-2xs font-medium tracking-[-0.01em]">SSH: {profile.name}</span>
            {conn.isConnected ? (
              <span className="flex items-center gap-1 text-3xs text-connection">
                <span className="h-1.5 w-1.5 rounded-full bg-connection" />
                {t('status.connected')}
              </span>
            ) : conn.isConnectingStatus || conn.isConnecting ? (
              <span className="flex items-center gap-1 text-3xs text-warning">
                <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-warning" />
                {t('status.connecting')}
              </span>
            ) : (
              <span className="flex items-center gap-1 text-3xs text-muted-foreground">
                <span className="h-1.5 w-1.5 rounded-full bg-muted-foreground/40" />
                {t('status.disconnected')}
              </span>
            )}
          </div>
          <div className="flex items-center gap-1">
            {conn.isConnected || conn.localTerminalPtyId ? (
              <button
                type="button"
                onClick={conn.handleDisconnect}
                className="inline-flex h-6 items-center gap-1 rounded-md border border-border/80 px-2 text-2xs text-muted-foreground transition-colors hover:border-destructive/40 hover:bg-destructive/10 hover:text-destructive focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <WifiOff className="h-3 w-3" />
                {t('connection.disconnect')}
              </button>
            ) : (
              <button
                type="button"
                onClick={conn.handleConnect}
                disabled={conn.isConnecting}
                className="inline-flex h-6 items-center gap-1 rounded-md bg-primary px-2 text-2xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-45"
              >
                <Terminal className="h-3 w-3" />
                {conn.isConnecting ? t('connection.connecting') : t('connection.connect')}
              </button>
            )}
          </div>
        </div>

        {/* Content area */}
        <div className="relative flex min-h-0 flex-1">
          {editingFile && conn.connectionId ? (
            <SSHFileEditor connectionId={conn.connectionId} />
          ) : editingFile && !conn.connectionId ? (
            <div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
              <div className="flex size-8 items-center justify-center rounded-md bg-secondary/50">
                <Terminal className="h-4 w-4 text-muted-foreground" />
              </div>
              <p className="text-sm font-medium text-foreground">
                {t('workspace.reconnectingEditor')}
              </p>
              <button
                type="button"
                onClick={conn.handleConnect}
                disabled={conn.isConnecting}
                className="inline-flex h-8 items-center gap-1.5 rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-45"
              >
                <Terminal className="h-3.5 w-3.5" />
                {conn.isConnecting ? t('connection.connecting') : t('connection.reconnect')}
              </button>
            </div>
          ) : conn.localTerminalPtyId ? (
            <div className="absolute inset-0 overflow-hidden">
              <ConnectedTerminal
                terminalId={conn.localTerminalPtyId}
                autoSpawn={false}
                isVisible={true}
                onExit={conn.handleSSHProcessExit}
              />
            </div>
          ) : (
            <div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
              <div className="flex size-8 items-center justify-center rounded-md bg-secondary/50">
                <Terminal className="h-4 w-4 text-muted-foreground" />
              </div>
              <div>
                <p className="text-sm font-medium text-foreground">{t('workspace.title')}</p>
                <p className="mt-1 text-xs text-muted-foreground">{t('workspace.description')}</p>
              </div>
              <button
                type="button"
                onClick={conn.handleConnect}
                disabled={conn.isConnecting}
                className="inline-flex h-8 items-center gap-1.5 rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-45"
              >
                <Terminal className="h-3.5 w-3.5" />
                {conn.isConnecting ? t('connection.connecting') : t('workspace.connectAndOpen')}
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
