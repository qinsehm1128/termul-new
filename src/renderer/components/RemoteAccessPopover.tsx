import type { RemotePublishMode } from '@shared/types/ipc.types'
import { AlertCircle, Check, Copy, Monitor, ShieldAlert } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import { toProjectGroupSummaries, toProjectSummaries } from '@/hooks/use-projects-persistence'
import { useSshTranslation } from '@/hooks/use-ssh-translation'
import { remoteServerApi, syncProjects } from '@/lib/api'
import { cn } from '@/lib/utils'
import { useProjectStore } from '@/stores/project-store'
import {
  useRemoteRestoreError,
  useRemoteStatus,
  useRemoteStatusStore
} from '@/stores/remote-status-store'

const statusBarTriggerClass =
  'flex h-5 cursor-pointer items-center rounded-sm px-1.5 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground'

function providerLabelKey(provider: string | null | undefined): string {
  if (provider === 'cloudflareNamed') return 'remote.providerNamed'
  if (provider === 'frp') return 'remote.providerFrp'
  if (provider === 'sshReverse') return 'remote.providerSsh'
  return 'remote.providerQuick'
}

/**
 * StatusBar popover for remote agent access.
 *
 * The switch is the operator wish (`wanted`). Publish mode chooses the QR:
 * same-Wi-Fi LAN or the configured public tunnel. Every published URL carries
 * `#access_token=`.
 */
export function RemoteAccessPopover(): React.JSX.Element {
  const t = useSshTranslation()
  const remoteStatus = useRemoteStatus()
  const restoreError = useRemoteRestoreError()
  const [remoteBusy, setRemoteBusy] = useState(false)
  const [remoteError, setRemoteError] = useState<string | null>(null)
  const [copiedUrl, setCopiedUrl] = useState(false)
  const [publishMode, setPublishMode] = useState<RemotePublishMode>('tunnel')

  const isRunning = remoteStatus?.running ?? false
  const runningRef = useRef(isRunning)
  runningRef.current = isRunning
  const lanAccessUrl = remoteStatus?.lanAccessUrl ?? null
  const tunnelAccessUrl = remoteStatus?.tunnelAccessUrl ?? remoteStatus?.accessUrl ?? null
  const selectedUrl = publishMode === 'lan' ? lanAccessUrl : tunnelAccessUrl
  const accessUrl = selectedUrl?.includes('#access_token=') ? selectedUrl : null
  const [sawTunnelUrl, setSawTunnelUrl] = useState(false)

  useEffect(() => {
    if (remoteStatus?.publishMode === 'lan' || remoteStatus?.publishMode === 'tunnel') {
      setPublishMode(remoteStatus.publishMode)
    }
  }, [remoteStatus?.publishMode])

  useEffect(() => {
    let cancelled = false
    void remoteServerApi.intent().then((result) => {
      if (cancelled || !result.success) return
      if (runningRef.current) return
      setPublishMode(result.data.publishMode)
    })
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    if (tunnelAccessUrl) setSawTunnelUrl(true)
    if (!isRunning) setSawTunnelUrl(false)
  }, [tunnelAccessUrl, isRunning])

  const bindModeFor = (mode: RemotePublishMode): 'localhost' | 'all' =>
    mode === 'lan' ? 'all' : 'localhost'

  const seedProjects = async (): Promise<void> => {
    const { projects, groups, activeProjectId } = useProjectStore.getState()
    const syncResult = await syncProjects(
      toProjectSummaries(projects, activeProjectId),
      activeProjectId || null,
      toProjectGroupSummaries(groups)
    )
    if (!syncResult.success) {
      toast.error(t('remote.seedProjectsFailed', { error: syncResult.error }))
    }
  }

  const handleRemoteToggle = async (enable: boolean): Promise<void> => {
    setRemoteBusy(true)
    setRemoteError(null)
    try {
      const result = enable
        ? await remoteServerApi.start({ bindMode: bindModeFor(publishMode) })
        : await remoteServerApi.stop()
      if (result.success) {
        useRemoteStatusStore.getState().setStatus(result.data)
        useRemoteStatusStore.getState().setRestoreError(null)
        if (enable) await seedProjects()
      } else {
        setRemoteError(result.error)
      }
    } catch (error) {
      setRemoteError(error instanceof Error ? error.message : String(error))
    } finally {
      setRemoteBusy(false)
    }
  }

  const handlePublishMode = async (mode: RemotePublishMode): Promise<void> => {
    setPublishMode(mode)
    const persisted = await remoteServerApi.setIntent({ publishMode: mode })
    if (!persisted.success) {
      setRemoteError(persisted.error)
      return
    }
    if (!isRunning) return
    const needsAll = mode === 'lan'
    const isAll = remoteStatus?.bindMode === 'all'
    if (needsAll === isAll && remoteStatus) {
      useRemoteStatusStore.getState().setStatus({
        ...remoteStatus,
        publishMode: mode,
        accessUrl: mode === 'lan' ? lanAccessUrl : tunnelAccessUrl
      })
      return
    }
    setRemoteBusy(true)
    setRemoteError(null)
    try {
      const stopped = await remoteServerApi.stop()
      if (!stopped.success) {
        setRemoteError(stopped.error)
        return
      }
      const started = await remoteServerApi.start({ bindMode: bindModeFor(mode) })
      if (started.success) {
        useRemoteStatusStore.getState().setStatus(started.data)
        await seedProjects()
      } else {
        useRemoteStatusStore.getState().setStatus(stopped.data)
        setRemoteError(started.error)
      }
    } finally {
      setRemoteBusy(false)
    }
  }

  const handleRotate = async (): Promise<void> => {
    setRemoteBusy(true)
    setRemoteError(null)
    try {
      const result = await remoteServerApi.rotateCredential()
      if (result.success) {
        useRemoteStatusStore.getState().setStatus(result.data)
        toast.success(t('remote.rotated'))
      } else {
        setRemoteError(result.error)
      }
    } finally {
      setRemoteBusy(false)
    }
  }

  const handleCopyLink = async (): Promise<void> => {
    if (!accessUrl) return
    try {
      await navigator.clipboard.writeText(accessUrl)
      setCopiedUrl(true)
      setTimeout(() => setCopiedUrl(false), 1500)
    } catch {
      // Clipboard unavailable; ignore.
    }
  }

  const displayError = remoteError ?? restoreError
  const listenerLabel = remoteStatus?.port
    ? t('remote.diagListener', { port: String(remoteStatus.port) })
    : t('remote.diagListenerOff')
  const lanLabel =
    remoteStatus?.bindMode === 'all' && remoteStatus.lanUrl
      ? t('remote.diagLan', { url: remoteStatus.lanUrl })
      : t('remote.diagLanLoopback')
  const tunnelLabel = remoteStatus?.tunnelUrl
    ? t('remote.diagTunnelUp', { provider: remoteStatus.tunnelProvider ?? 'tunnel' })
    : t('remote.diagTunnelDown')

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={statusBarTriggerClass}
          aria-label={t('remote.aria')}
          aria-pressed={isRunning}
        >
          <Monitor size={14} className={cn('mr-0', isRunning ? 'text-connection' : undefined)} />
          {isRunning && <span className="sr-only">{t('remote.enabled')}</span>}
        </button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="end"
        className="w-80 p-3 shadow-[0_8px_24px_hsl(var(--background)/0.55),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
      >
        <div className="space-y-2.5">
          <div>
            <h4 className="text-xs font-medium tracking-[-0.01em] text-foreground">
              {t('remote.title')}
            </h4>
            <p className="mt-1 text-2xs leading-relaxed text-muted-foreground">
              {t('remote.description')}
            </p>
          </div>

          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="text-xs text-foreground">{t('remote.enable')}</div>
              <div className="mt-0.5 text-2xs text-muted-foreground">{t('remote.enableHint')}</div>
            </div>
            <Switch
              checked={isRunning}
              disabled={remoteBusy}
              onCheckedChange={(checked) => void handleRemoteToggle(checked)}
              aria-label={t('remote.toggleAria')}
            />
          </div>

          <div
            className="grid grid-cols-2 gap-1 rounded-md border border-border/80 bg-secondary/30 p-0.5"
            role="tablist"
            aria-label={t('remote.publishModeAria')}
          >
            {(['lan', 'tunnel'] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                role="tab"
                aria-selected={publishMode === mode}
                disabled={remoteBusy}
                onClick={() => void handlePublishMode(mode)}
                className={cn(
                  'h-7 rounded-sm px-2 text-2xs font-medium transition-colors',
                  publishMode === mode
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'
                )}
              >
                {mode === 'lan' ? t('remote.modeLan') : t('remote.modeTunnel')}
              </button>
            ))}
          </div>

          {displayError && (
            <div className="flex items-start gap-2 rounded-md border border-destructive/35 bg-destructive/10 px-2.5 py-2 text-xs text-destructive">
              <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              <span>{displayError}</span>
            </div>
          )}

          {isRunning && (
            <div className="space-y-0.5 text-2xs text-muted-foreground">
              <p>{listenerLabel}</p>
              <p>{lanLabel}</p>
              <p>{tunnelLabel}</p>
            </div>
          )}

          {isRunning && accessUrl && (
            <div className="space-y-2">
              <div className="flex justify-center">
                <div className="rounded-md bg-white p-1.5">
                  <QRCodeSVG value={accessUrl} size={160} level="M" />
                </div>
              </div>
              <p className="text-2xs text-muted-foreground">
                {publishMode === 'lan'
                  ? t('remote.lanHint')
                  : t(providerLabelKey(remoteStatus?.tunnelProvider))}
              </p>
              <div className="flex items-start gap-2 rounded-md border border-warning/35 bg-warning/10 px-2.5 py-2 text-2xs text-warning">
                <ShieldAlert className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                <span>
                  {publishMode === 'lan'
                    ? t('remote.lanSecurityWarning')
                    : t('remote.securityWarning')}
                </span>
              </div>
              <button
                type="button"
                onClick={() => void handleCopyLink()}
                className="inline-flex h-7 w-full items-center justify-center gap-1.5 rounded-md border border-border/80 bg-secondary/50 px-2.5 text-2xs font-medium text-foreground transition-colors hover:bg-secondary focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                aria-label={t('remote.copyAria')}
              >
                {copiedUrl ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
                {copiedUrl ? t('remote.copied') : t('remote.copy')}
              </button>
              <button
                type="button"
                disabled={remoteBusy}
                onClick={() => void handleRotate()}
                className="inline-flex h-7 w-full items-center justify-center rounded-md px-2.5 text-2xs text-muted-foreground transition-colors hover:text-foreground"
              >
                {t('remote.rotate')}
              </button>
            </div>
          )}

          {isRunning && !accessUrl && (
            <div className="flex flex-col items-center justify-center gap-1.5 py-4 text-center">
              <span
                className={cn(
                  'h-1.5 w-1.5 rounded-full',
                  sawTunnelUrl ? 'bg-warning' : 'animate-pulse bg-connection'
                )}
              />
              <p
                className={cn('text-2xs', sawTunnelUrl ? 'text-warning' : 'text-muted-foreground')}
              >
                {publishMode === 'lan'
                  ? t('remote.lanUnavailable')
                  : sawTunnelUrl
                    ? t('remote.disconnected')
                    : t('remote.starting')}
              </p>
            </div>
          )}
        </div>
      </PopoverContent>
    </Popover>
  )
}
