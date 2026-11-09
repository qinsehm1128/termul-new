import type { RemotePairingCode } from '@shared/remote-devices'
import type { RemotePublishMode } from '@shared/types/ipc.types'
import { Check, Copy, Monitor, RefreshCw } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { DeviceManagementPanel } from '@/components/DeviceManagementPanel'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import { toProjectGroupSummaries, toProjectSummaries } from '@/hooks/use-projects-persistence'
import { useSshTranslation } from '@/hooks/use-ssh-translation'
import { remoteServerApi, syncProjects } from '@/lib/api'
import { logFrontendError } from '@/lib/log-api'
import { remoteDevicesApi } from '@/lib/remote-devices-api'
import { cn } from '@/lib/utils'
import { useProjectStore } from '@/stores/project-store'
import {
  useRemoteRestoreError,
  useRemoteStatus,
  useRemoteStatusStore
} from '@/stores/remote-status-store'

/** Drop all legacy credentials, including fragments, queries and URL userinfo. */
function pairingOrigin(value: string | null | undefined): string | null {
  if (!value) return null
  try {
    const url = new URL(value)
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.origin : null
  } catch {
    return null
  }
}

export function RemoteAccessPopover(): React.JSX.Element {
  const t = useSshTranslation()
  const remoteStatus = useRemoteStatus()
  const restoreError = useRemoteRestoreError()
  const [open, setOpen] = useState(false)
  const [remoteBusy, setRemoteBusy] = useState(false)
  const [remoteError, setRemoteError] = useState<string | null>(null)
  const [copiedUrl, setCopiedUrl] = useState(false)
  const [publishMode, setPublishMode] = useState<RemotePublishMode>('tunnel')
  const [pairing, setPairing] = useState<
    (RemotePairingCode & { origin: string; version: number }) | null
  >(null)
  const [pairingError, setPairingError] = useState(false)
  const [pairingVersion, setPairingVersion] = useState(0)
  const [now, setNow] = useState(Date.now())
  const isRunning = remoteStatus?.running ?? false
  const runningRef = useRef(isRunning)
  runningRef.current = isRunning
  const lanAccessUrl = remoteStatus?.lanUrl ?? remoteStatus?.lanAccessUrl ?? null
  const tunnelAccessUrl =
    remoteStatus?.tunnelUrl ?? remoteStatus?.tunnelAccessUrl ?? remoteStatus?.accessUrl ?? null
  const origin = isRunning
    ? pairingOrigin(publishMode === 'lan' ? lanAccessUrl : tunnelAccessUrl)
    : null

  useEffect(() => {
    if (remoteStatus?.publishMode === 'lan' || remoteStatus?.publishMode === 'tunnel')
      setPublishMode(remoteStatus.publishMode)
  }, [remoteStatus?.publishMode])
  useEffect(() => {
    let cancelled = false
    void Promise.resolve(remoteServerApi.intent()).then((result) => {
      if (!cancelled && result?.success && result.data && !runningRef.current)
        setPublishMode(result.data.publishMode)
    })
    return () => {
      cancelled = true
    }
  }, [])
  useEffect(() => {
    if (!open || !origin) {
      setPairing(null)
      return
    }
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const issue = async (): Promise<void> => {
      setPairing(null)
      setPairingError(false)
      try {
        const next = await remoteDevicesApi.pairingCode()
        if (cancelled) return
        if (!next.code || next.expiresAt * 1000 <= Date.now()) throw new Error('Expired code')
        setPairing({ ...next, origin, version: pairingVersion })
        setNow(Date.now())
        timer = setTimeout(() => void issue(), Math.min(60000, next.expiresAt * 1000 - Date.now()))
      } catch {
        if (!cancelled) {
          setPairingError(true)
          void logFrontendError({
            source: 'RemoteAccessPopover',
            message: 'pairing code unavailable'
          })
        }
      }
    }
    void issue()
    return () => {
      cancelled = true
      if (timer) clearTimeout(timer)
    }
  }, [open, origin, pairingVersion])
  useEffect(() => {
    if (!open) return
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [open])
  const accessUrl =
    origin &&
    pairing &&
    pairing.origin === origin &&
    pairing.version === pairingVersion &&
    pairing.expiresAt * 1000 > now
      ? `${origin}/#pair=${encodeURIComponent(pairing.code)}`
      : null

  const seedProjects = async (): Promise<void> => {
    const { projects, groups, activeProjectId } = useProjectStore.getState()
    const result = await syncProjects(
      toProjectSummaries(projects, activeProjectId),
      activeProjectId || null,
      toProjectGroupSummaries(groups)
    )
    if (!result.success) toast.error(t('remote.seedProjectsFailed', { error: result.error }))
  }
  const bindModeFor = (mode: RemotePublishMode): 'localhost' | 'all' =>
    mode === 'lan' ? 'all' : 'localhost'
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
      } else setRemoteError('远程服务切换失败')
    } catch {
      setRemoteError('远程服务切换失败')
    } finally {
      setRemoteBusy(false)
    }
  }
  const handlePublishMode = async (mode: RemotePublishMode): Promise<void> => {
    setRemoteBusy(true)
    setRemoteError(null)
    setPairing(null)
    try {
      const persisted = await remoteServerApi.setIntent({ publishMode: mode })
      if (!persisted.success) {
        setRemoteError('访问方式切换失败')
        return
      }
      setPublishMode(mode)
      if (!isRunning) return
      if ((mode === 'lan') === (remoteStatus?.bindMode === 'all') && remoteStatus) {
        useRemoteStatusStore.getState().setStatus({
          ...remoteStatus,
          publishMode: mode,
          accessUrl: mode === 'lan' ? lanAccessUrl : tunnelAccessUrl
        })
        return
      }
      const stopped = await remoteServerApi.stop()
      if (!stopped.success) {
        setRemoteError('远程服务停止失败')
        return
      }
      const started = await remoteServerApi.start({ bindMode: bindModeFor(mode) })
      if (started.success) {
        useRemoteStatusStore.getState().setStatus(started.data)
        await seedProjects()
      } else {
        useRemoteStatusStore.getState().setStatus(stopped.data)
        setRemoteError('远程服务启动失败')
      }
    } catch {
      setRemoteError('访问方式切换失败')
    } finally {
      setRemoteBusy(false)
    }
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="flex h-5 cursor-pointer items-center rounded-sm px-1.5 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
          aria-label={t('remote.aria')}
          aria-pressed={isRunning}
        >
          <Monitor size={14} className={cn(isRunning && 'text-connection')} />
        </button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="end"
        className="max-h-[85vh] w-96 space-y-3 overflow-y-auto p-3"
      >
        <div className="flex items-center justify-between">
          <h4 className="text-xs font-medium">远程访问</h4>
          <Switch
            checked={isRunning}
            disabled={remoteBusy}
            onCheckedChange={(checked) => void handleRemoteToggle(checked)}
            aria-label={t('remote.toggleAria')}
          />
        </div>
        <div
          className="grid grid-cols-2 gap-1 rounded-md border border-border bg-secondary/30 p-0.5"
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
                'h-7 rounded-sm px-2 text-2xs',
                publishMode === mode
                  ? 'bg-background text-foreground shadow-sm'
                  : 'text-muted-foreground'
              )}
            >
              {mode === 'lan' ? '局域网' : '公网'}
            </button>
          ))}
        </div>
        {(remoteError || restoreError) && (
          <div role="alert" className="text-xs text-destructive">
            {remoteError || '远程服务恢复失败'}
          </div>
        )}
        {isRunning && accessUrl && (
          <div className="space-y-2">
            <div className="flex justify-center">
              <div className="rounded-md bg-white p-1.5">
                <QRCodeSVG value={accessUrl} size={180} level="M" />
              </div>
            </div>
            <div className="flex items-center justify-between text-2xs text-muted-foreground">
              <span>
                一次性配对 ·{' '}
                {Math.max(0, Math.ceil(((pairing?.expiresAt ?? 0) * 1000 - now) / 1000))} 秒
              </span>
              <button
                type="button"
                className="flex items-center gap-1 hover:text-foreground"
                onClick={() => {
                  setPairing(null)
                  setPairingVersion((value) => value + 1)
                }}
              >
                <RefreshCw className="size-3" />
                刷新
              </button>
            </div>
            <button
              type="button"
              aria-label={t('remote.copyAria')}
              className="inline-flex h-7 w-full items-center justify-center gap-1.5 rounded-md border border-border bg-secondary/50 text-2xs"
              onClick={() => {
                void navigator.clipboard
                  .writeText(accessUrl)
                  .then(() => {
                    setCopiedUrl(true)
                    setTimeout(() => setCopiedUrl(false), 1500)
                  })
                  .catch(() => toast.error('复制失败'))
              }}
            >
              {copiedUrl ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
              {copiedUrl ? '已复制' : '复制配对链接'}
            </button>
          </div>
        )}
        {isRunning && !accessUrl && (
          <div role="status" className="py-3 text-center text-2xs text-muted-foreground">
            {pairingError ? (
              <button type="button" onClick={() => setPairingVersion((value) => value + 1)}>
                配对码生成失败 · 重试
              </button>
            ) : origin ? (
              '正在生成配对码…'
            ) : publishMode === 'lan' ? (
              '暂无局域网地址'
            ) : (
              '等待公网地址…'
            )}
          </div>
        )}
        {open && (
          <DeviceManagementPanel
            onCredentialsRevoked={() => {
              setPairing(null)
              setPairingVersion((value) => value + 1)
            }}
          />
        )}
      </PopoverContent>
    </Popover>
  )
}
