import {
  REMOTE_DEVICE_SCOPES,
  type RemoteDevice,
  type RemoteDeviceAuditEntry,
  type RemoteDeviceScope
} from '@shared/remote-devices'
import { useCallback, useEffect, useRef, useState } from 'react'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogTitle
} from '@/components/ui/alert-dialog'
import { remoteDevicesApi } from '@/lib/remote-devices-api'
import { useProjectStore } from '@/stores/project-store'

const scopeLabels: Record<RemoteDeviceScope, string> = {
  view: '查看',
  'terminal.input': '终端输入',
  'terminal.create': '新建终端',
  'agent.run': '运行 Agent',
  'files.write': '修改文件',
  'git.write': 'Git 写入',
  'screen.view': '查看屏幕',
  'screen.control': '控制屏幕',
  'plugins.use': '使用插件'
}
const buttonClass =
  'rounded border border-border px-2 py-1 text-2xs hover:bg-secondary disabled:opacity-50'
const auditLabels: Record<string, string> = {
  device_pair: '设备配对',
  device_revoke: '撤销配对',
  device_disconnect: '断开连接',
  scope_denied: '权限拒绝'
}
const timeLabel = (at: number): string => new Date(at * 1000).toLocaleString()

function DeviceRow({
  device,
  busy,
  run
}: {
  device: RemoteDevice
  busy: boolean
  run: (operation: () => Promise<void>) => Promise<void>
}): React.JSX.Element {
  const [name, setName] = useState(device.name)
  useEffect(() => setName(device.name), [device.name])
  const revoked = device.revokedAt != null
  const editable = !busy && !revoked && !device.legacy
  return (
    <li className="space-y-2 rounded-md border border-border p-2" aria-label={device.name}>
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-xs font-medium" title={device.platform}>
          {device.name}
        </span>
        <span className="shrink-0 text-2xs text-muted-foreground">
          {revoked
            ? '已撤销'
            : device.connectionCount > 0
              ? `在线 · ${device.connectionCount} 个连接`
              : '离线'}
        </span>
      </div>
      {device.legacy && (
        <div className="text-2xs text-destructive" role="note">
          旧版主机凭据 · 不可单独撤销
        </div>
      )}
      <details className="text-2xs">
        <summary className="cursor-pointer text-muted-foreground">设备与权限</summary>
        <div className="mt-2 space-y-2">
          <div className="text-muted-foreground">
            {device.platform} · 最近活跃 {timeLabel(device.lastSeen)}
          </div>
          <div className="text-muted-foreground">配对于 {timeLabel(device.createdAt)}</div>
          {!device.legacy && (
            <form
              className="flex gap-1"
              onSubmit={(event) => {
                event.preventDefault()
                if (editable && name.trim())
                  void run(() => remoteDevicesApi.rename(device.id, name.trim()))
              }}
            >
              <input
                aria-label={`${device.name} 名称`}
                className="min-w-0 flex-1 rounded border border-border bg-background px-2 py-1"
                value={name}
                disabled={!editable}
                maxLength={128}
                onChange={(event) => setName(event.target.value)}
              />
              <button
                type="submit"
                className={buttonClass}
                disabled={!editable || !name.trim() || name.trim() === device.name}
              >
                保存
              </button>
            </form>
          )}
          <fieldset
            disabled={!editable}
            className="grid grid-cols-2 gap-2"
            aria-label={`${device.name} 权限`}
          >
            {REMOTE_DEVICE_SCOPES.map((scope) => (
              <label key={scope} className="flex items-center gap-1.5">
                <input
                  type="checkbox"
                  checked={device.scopes.includes(scope)}
                  onChange={(event) => {
                    const scopes = event.target.checked
                      ? [...device.scopes, scope]
                      : device.scopes.filter((item) => item !== scope)
                    void run(() => remoteDevicesApi.setPermissions(device.id, scopes))
                  }}
                />
                {scopeLabels[scope]}
              </label>
            ))}
          </fieldset>
          {!device.legacy && (
            <button
              type="button"
              className={buttonClass}
              disabled={!editable}
              title="授予全部设备权限，不含主机管理"
              onClick={() => void run(() => remoteDevicesApi.grantAll(device.id))}
            >
              全部授权
            </button>
          )}
        </div>
      </details>
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          className={buttonClass}
          disabled={busy || revoked || device.connectionCount === 0}
          title="仅关闭当前连接，设备可重新连接"
          onClick={() => void run(() => remoteDevicesApi.disconnect(device.id))}
        >
          断开连接
        </button>
        {!device.legacy && (
          <button
            type="button"
            className={`${buttonClass} text-destructive`}
            disabled={busy || revoked}
            title="撤销凭据，设备必须重新配对"
            onClick={() => void run(() => remoteDevicesApi.revoke(device.id))}
          >
            撤销配对
          </button>
        )}
      </div>
    </li>
  )
}

/** Mounted only while the desktop remote-access popover is open. */
export function DeviceManagementPanel({
  onCredentialsRevoked
}: {
  onCredentialsRevoked?: () => void
} = {}): React.JSX.Element {
  const projectId = useProjectStore((state) => state.activeProjectId)
  const [devices, setDevices] = useState<RemoteDevice[]>([])
  const [allProjects, setAllProjects] = useState(false)
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [confirmAll, setConfirmAll] = useState(false)
  const [showAudit, setShowAudit] = useState(false)
  const [audit, setAudit] = useState<RemoteDeviceAuditEntry[]>([])
  const mounted = useRef(false)
  const sequence = useRef(0)
  const refresh = useCallback(async (): Promise<void> => {
    const current = ++sequence.current
    try {
      const result = await remoteDevicesApi.list()
      if (mounted.current && sequence.current === current) {
        setDevices(result.devices)
        setError(null)
      }
    } catch {
      if (mounted.current && sequence.current === current) setError('设备列表加载失败')
    } finally {
      if (mounted.current && sequence.current === current) setLoading(false)
    }
  }, [])
  useEffect(() => {
    mounted.current = true
    void refresh()
    const timer = setInterval(() => void refresh(), 5000)
    return () => {
      mounted.current = false
      clearInterval(timer)
    }
  }, [refresh])
  const run = async (operation: () => Promise<void>): Promise<void> => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      await operation()
      await refresh()
      if (showAudit) setAudit(await remoteDevicesApi.audit())
    } catch {
      if (mounted.current) setError('设备操作失败，请重试')
    } finally {
      if (mounted.current) setBusy(false)
    }
  }
  const visible = devices.filter((device) => allProjects || device.projectId === projectId)
  return (
    <section className="space-y-2 border-t border-border pt-3" aria-label="设备管理">
      <div className="flex items-center justify-between gap-2">
        <h4 className="text-xs font-medium">设备管理</h4>
        <label className="flex items-center gap-1.5 text-2xs text-muted-foreground">
          <input
            type="checkbox"
            aria-label="全部设备"
            checked={allProjects}
            onChange={(event) => setAllProjects(event.target.checked)}
          />
          全部设备 ({devices.length})
        </label>
      </div>
      {error && (
        <div role="alert" className="text-2xs text-destructive">
          {error}
        </div>
      )}
      {loading ? (
        <div role="status" className="text-2xs text-muted-foreground">
          加载中…
        </div>
      ) : visible.length === 0 ? (
        <div className="text-2xs text-muted-foreground">暂无设备</div>
      ) : (
        <ul className="max-h-72 space-y-2 overflow-y-auto">
          {visible.map((device) => (
            <DeviceRow key={device.id} device={device} busy={busy} run={run} />
          ))}
        </ul>
      )}
      <div className="flex justify-between gap-2">
        <button
          type="button"
          className={buttonClass}
          disabled={busy}
          onClick={() => {
            if (showAudit) {
              setShowAudit(false)
              return
            }
            void run(async () => {
              setAudit(await remoteDevicesApi.audit())
              setShowAudit(true)
            })
          }}
        >
          {showAudit ? '收起记录' : '操作记录'}
        </button>
        <button
          type="button"
          className={`${buttonClass} text-destructive`}
          disabled={busy}
          onClick={() => setConfirmAll(true)}
        >
          强制断开全部
        </button>
      </div>
      {showAudit && (
        <ul
          aria-label="操作记录"
          className="max-h-32 space-y-1 overflow-y-auto text-2xs text-muted-foreground"
        >
          {audit
            .slice()
            .reverse()
            .map((entry, index) => (
              <li key={`${entry.at}-${index}`}>
                {timeLabel(entry.at)} · {auditLabels[entry.event] ?? entry.event} ·{' '}
                {devices.find((device) => device.id === entry.deviceId)?.name ?? entry.deviceId}
                {entry.capability ? ` · ${entry.capability}` : ''}
              </li>
            ))}
        </ul>
      )}
      <AlertDialog open={confirmAll} onOpenChange={setConfirmAll}>
        <AlertDialogContent>
          <AlertDialogTitle>强制断开全部设备？</AlertDialogTitle>
          <AlertDialogDescription>
            撤销全部设备及旧版主机凭据，所有设备必须重新配对。
          </AlertDialogDescription>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              onClick={() =>
                void run(async () => {
                  await remoteDevicesApi.disconnectAll()
                  onCredentialsRevoked?.()
                })
              }
            >
              撤销全部并断开
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  )
}
