import { useCallback, useEffect, useRef, useState } from 'react'
import { Switch } from '@/components/ui/switch'
import { openPrivacyPane, type PermissionId } from '@/lib/macos-permissions-api'
import {
  type RemoteDesktopSettings as DesktopSettings,
  remoteDesktopApi
} from '@/lib/remote-desktop-api'
import { isTauriContext } from '@/lib/tauri-runtime'

/** Desktop consent only. Browsers cannot grant access to the remote host. */
export function RemoteDesktopSettings(): React.JSX.Element {
  const desktop = isTauriContext()
  const [settings, setSettings] = useState<DesktopSettings | null>(null)
  const [loading, setLoading] = useState(desktop)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const mounted = useRef(false)
  const busy = useRef(false)

  const refresh = useCallback(async () => {
    if (!desktop || busy.current) return
    busy.current = true
    setLoading(true)
    try {
      const next = await remoteDesktopApi.getSettings()
      if (mounted.current) {
        setSettings(next)
        setError(null)
      }
    } catch {
      if (mounted.current) setError('无法读取远程桌面设置，请重试')
    } finally {
      busy.current = false
      if (mounted.current) setLoading(false)
    }
  }, [desktop])

  useEffect(() => {
    mounted.current = true
    void refresh()
    const onFocus = () => void refresh()
    window.addEventListener('focus', onFocus)
    return () => {
      mounted.current = false
      window.removeEventListener('focus', onFocus)
    }
  }, [refresh])

  const update = async (key: 'viewEnabled' | 'controlEnabled', value: boolean) => {
    if (!desktop || !settings?.supported || busy.current) return
    busy.current = true
    setSaving(true)
    setError(null)
    try {
      // Persist both independent switches; never silently grant control with viewing.
      const next = await remoteDesktopApi.setSettings({
        viewEnabled: settings.viewEnabled,
        controlEnabled: settings.controlEnabled,
        [key]: value
      })
      if (mounted.current) setSettings(next)
    } catch {
      if (mounted.current) setError('无法保存远程桌面设置，请重试')
    } finally {
      busy.current = false
      if (mounted.current) setSaving(false)
    }
  }

  const openPermission = async (id: PermissionId) => {
    try {
      await openPrivacyPane(id)
    } catch {
      if (mounted.current) setError('无法打开系统设置，请手动检查隐私权限')
    }
  }

  const unavailable = !desktop || (settings !== null && !settings.supported)
  const disabled = unavailable || loading || saving || !settings
  return (
    <section className="space-y-3" aria-label="远程桌面">
      <h3 className="text-sm font-medium">远程桌面</h3>
      {unavailable ? (
        <p className="text-xs text-muted-foreground">
          {desktop ? '此设备不支持远程桌面' : '请在桌面客户端设置远程桌面，浏览器不支持此功能'}
        </p>
      ) : (
        <>
          <div className="flex items-center justify-between gap-3">
            <label htmlFor="remote-desktop-view" className="text-sm">
              允许查看桌面
            </label>
            <Switch
              id="remote-desktop-view"
              checked={settings?.viewEnabled ?? true}
              disabled={disabled}
              onCheckedChange={(value) => void update('viewEnabled', value)}
              title="已配对设备可查看屏幕"
            />
          </div>
          <div className="flex items-center justify-between gap-3">
            <label htmlFor="remote-desktop-control" className="text-sm">
              允许控制鼠标键盘
            </label>
            <Switch
              id="remote-desktop-control"
              checked={settings?.controlEnabled ?? false}
              disabled={disabled}
              onCheckedChange={(value) => void update('controlEnabled', value)}
              title="默认关闭；可随时停止"
            />
          </div>
          {settings?.controlEnabled && (
            <p className="text-xs text-muted-foreground">已配对设备可操作这台电脑</p>
          )}
          {settings?.supported && (
            <div className="space-y-2 rounded-md border border-border p-3">
              {(
                [
                  ['screenRecording', '屏幕录制'],
                  ['accessibility', '辅助功能']
                ] as const
              ).map(([id, label]) => (
                <div key={id} className="flex items-center justify-between gap-3 text-xs">
                  <span>
                    {label} · {settings.permissions[id] ? '已授权' : '未授权'}
                  </span>
                  <button
                    type="button"
                    className="underline text-muted-foreground"
                    onClick={() => void openPermission(id)}
                    title={`在系统设置中检查${label}权限`}
                    aria-label={`打开${label}系统设置`}
                  >
                    系统设置
                  </button>
                </div>
              ))}
            </div>
          )}
        </>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      {desktop && !unavailable && (
        <button
          type="button"
          disabled={loading || saving}
          onClick={() => void refresh()}
          className="text-xs text-muted-foreground underline"
        >
          {loading ? '检查中…' : '重新检查权限'}
        </button>
      )}
    </section>
  )
}
