import type {
  TunnelConfigUpdate,
  TunnelConfigView,
  TunnelProviderKind
} from '@shared/types/ipc.types'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { tunnelConfigApi } from '@/lib/api'
import { isTauriContext } from '@/lib/tauri-runtime'

const PROVIDERS: TunnelProviderKind[] = ['cloudflareQuick', 'cloudflareNamed', 'frp', 'sshReverse']

const EMPTY_VIEW: TunnelConfigView = {
  provider: 'cloudflareQuick',
  cloudflareNamedHostname: null,
  cloudflareNamedLocalPort: null,
  cloudflareNamedTokenSet: false,
  frpServerAddr: null,
  frpServerPort: null,
  frpCustomDomain: null,
  frpRemotePort: null,
  frpPublicHttps: true,
  frpTokenSet: false,
  sshHost: null,
  sshPort: null,
  sshUser: null,
  sshRemotePort: null,
  sshPublicHostname: null,
  sshPublicHttps: true,
  sshPrivateKeySet: false
}

export function RemoteAccessSettings(): React.JSX.Element {
  const { t } = useTranslation('settings')
  const desktop = isTauriContext()
  const [view, setView] = useState<TunnelConfigView>(EMPTY_VIEW)
  const [hostname, setHostname] = useState('')
  const [namedPort, setNamedPort] = useState('')
  const [namedToken, setNamedToken] = useState('')
  const [frpAddr, setFrpAddr] = useState('')
  const [frpPort, setFrpPort] = useState('')
  const [frpDomain, setFrpDomain] = useState('')
  const [frpRemotePort, setFrpRemotePort] = useState('')
  const [frpToken, setFrpToken] = useState('')
  const [sshHost, setSshHost] = useState('')
  const [sshPort, setSshPort] = useState('')
  const [sshUser, setSshUser] = useState('')
  const [sshRemotePort, setSshRemotePort] = useState('')
  const [sshPublicHostname, setSshPublicHostname] = useState('')
  const [sshPrivateKey, setSshPrivateKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [loaded, setLoaded] = useState(false)

  const applyView = useCallback((next: TunnelConfigView) => {
    setView(next)
    setHostname(next.cloudflareNamedHostname ?? '')
    setNamedPort(next.cloudflareNamedLocalPort ? String(next.cloudflareNamedLocalPort) : '')
    setFrpAddr(next.frpServerAddr ?? '')
    setFrpPort(next.frpServerPort ? String(next.frpServerPort) : '')
    setFrpDomain(next.frpCustomDomain ?? '')
    setFrpRemotePort(next.frpRemotePort ? String(next.frpRemotePort) : '')
    setSshHost(next.sshHost ?? '')
    setSshPort(next.sshPort ? String(next.sshPort) : '')
    setSshUser(next.sshUser ?? '')
    setSshRemotePort(next.sshRemotePort ? String(next.sshRemotePort) : '')
    setSshPublicHostname(next.sshPublicHostname ?? '')
    setNamedToken('')
    setFrpToken('')
    setSshPrivateKey('')
  }, [])

  useEffect(() => {
    if (!desktop) {
      setLoaded(true)
      return
    }
    let cancelled = false
    void tunnelConfigApi.get().then((result) => {
      if (cancelled) return
      if (result.success) applyView(result.data)
      else toast.error(result.error ?? t('remoteAccess.loadFailed'))
      setLoaded(true)
    })
    return () => {
      cancelled = true
    }
  }, [applyView, desktop, t])

  const persist = async (update: TunnelConfigUpdate): Promise<void> => {
    if (!desktop) return
    setBusy(true)
    try {
      const result = await tunnelConfigApi.set(update)
      if (result.success) {
        applyView(result.data)
        toast.success(t('remoteAccess.saved'))
      } else {
        toast.error(result.error ?? t('remoteAccess.saveFailed'))
      }
    } finally {
      setBusy(false)
    }
  }

  const saveProvider = (provider: TunnelProviderKind): void => {
    void persist({
      provider,
      cloudflareNamedHostname: hostname,
      cloudflareNamedLocalPort: namedPort ? Number(namedPort) : null,
      frpServerAddr: frpAddr,
      frpServerPort: frpPort ? Number(frpPort) : null,
      frpCustomDomain: frpDomain,
      frpRemotePort: frpRemotePort ? Number(frpRemotePort) : null,
      frpPublicHttps: view.frpPublicHttps,
      sshHost,
      sshPort: sshPort ? Number(sshPort) : null,
      sshUser,
      sshRemotePort: sshRemotePort ? Number(sshRemotePort) : null,
      sshPublicHostname,
      sshPublicHttps: view.sshPublicHttps
    })
  }

  const saveDetails = (): void => {
    void persist({
      provider: view.provider,
      cloudflareNamedHostname: hostname,
      cloudflareNamedLocalPort: namedPort ? Number(namedPort) : null,
      cloudflareNamedToken: namedToken.trim() ? namedToken.trim() : undefined,
      frpServerAddr: frpAddr,
      frpServerPort: frpPort ? Number(frpPort) : null,
      frpCustomDomain: frpDomain,
      frpRemotePort: frpRemotePort ? Number(frpRemotePort) : null,
      frpPublicHttps: view.frpPublicHttps,
      frpToken: frpToken.trim() ? frpToken.trim() : undefined,
      sshHost,
      sshPort: sshPort ? Number(sshPort) : null,
      sshUser,
      sshRemotePort: sshRemotePort ? Number(sshRemotePort) : null,
      sshPublicHostname,
      sshPublicHttps: view.sshPublicHttps,
      sshPrivateKey: sshPrivateKey.trim() ? sshPrivateKey.trim() : undefined
    })
  }

  if (!desktop) {
    return <p className="text-sm text-muted-foreground">{t('remoteAccess.desktopOnly')}</p>
  }

  if (!loaded) {
    return <p className="text-sm text-muted-foreground">{t('remoteAccess.loading')}</p>
  }

  return (
    <div className="space-y-5">
      <div>
        <label
          className="block text-sm font-medium text-secondary-foreground mb-2"
          htmlFor="tunnel-provider"
        >
          {t('remoteAccess.provider')}
        </label>
        <select
          id="tunnel-provider"
          value={view.provider}
          disabled={busy}
          onChange={(event) => saveProvider(event.target.value as TunnelProviderKind)}
          className="w-full bg-secondary/50 border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:ring-2 focus:ring-primary focus:border-transparent outline-none"
        >
          {PROVIDERS.map((provider) => (
            <option key={provider} value={provider}>
              {t(`remoteAccess.providers.${provider}`)}
            </option>
          ))}
        </select>
        <p className="text-xs text-muted-foreground mt-1">
          {t(`remoteAccess.hints.${view.provider}`)}
        </p>
      </div>

      {view.provider === 'cloudflareNamed' && (
        <div className="space-y-3">
          <Field
            id="cf-hostname"
            label={t('remoteAccess.namedHostname')}
            hint={t('remoteAccess.namedHostnameHint')}
            value={hostname}
            onChange={setHostname}
            placeholder="termul.example.com"
          />
          <Field
            id="cf-local-port"
            label={t('remoteAccess.namedLocalPort')}
            hint={t('remoteAccess.namedLocalPortHint')}
            value={namedPort}
            onChange={setNamedPort}
            placeholder="18787"
            inputMode="numeric"
          />
          <Field
            id="cf-token"
            label={t('remoteAccess.namedToken')}
            hint={
              view.cloudflareNamedTokenSet
                ? t('remoteAccess.tokenSet')
                : t('remoteAccess.namedTokenHint')
            }
            value={namedToken}
            onChange={setNamedToken}
            type="password"
            autoComplete="off"
          />
        </div>
      )}

      {view.provider === 'frp' && (
        <div className="space-y-3">
          <Field
            id="frp-addr"
            label={t('remoteAccess.frpServerAddr')}
            hint={t('remoteAccess.frpServerAddrHint')}
            value={frpAddr}
            onChange={setFrpAddr}
            placeholder="vps.example.com"
          />
          <Field
            id="frp-port"
            label={t('remoteAccess.frpServerPort')}
            hint={t('remoteAccess.frpServerPortHint')}
            value={frpPort}
            onChange={setFrpPort}
            placeholder="7000"
            inputMode="numeric"
          />
          <Field
            id="frp-domain"
            label={t('remoteAccess.frpCustomDomain')}
            hint={t('remoteAccess.frpCustomDomainHint')}
            value={frpDomain}
            onChange={setFrpDomain}
            placeholder="termul.example.com"
          />
          <Field
            id="frp-remote-port"
            label={t('remoteAccess.frpRemotePort')}
            hint={t('remoteAccess.frpRemotePortHint')}
            value={frpRemotePort}
            onChange={setFrpRemotePort}
            placeholder="8443"
            inputMode="numeric"
          />
          <Field
            id="frp-token"
            label={t('remoteAccess.frpToken')}
            hint={view.frpTokenSet ? t('remoteAccess.tokenSet') : t('remoteAccess.frpTokenHint')}
            value={frpToken}
            onChange={setFrpToken}
            type="password"
            autoComplete="off"
          />
          <div className="flex items-center justify-between gap-3">
            <div>
              <div className="text-sm text-foreground">{t('remoteAccess.frpPublicHttps')}</div>
              <p className="text-xs text-muted-foreground mt-0.5">
                {t('remoteAccess.frpPublicHttpsHint')}
              </p>
            </div>
            <Switch
              checked={view.frpPublicHttps}
              disabled={busy}
              onCheckedChange={(checked) => {
                setView((current) => ({ ...current, frpPublicHttps: checked }))
                void persist({
                  provider: 'frp',
                  cloudflareNamedHostname: hostname,
                  frpServerAddr: frpAddr,
                  frpServerPort: frpPort ? Number(frpPort) : null,
                  frpCustomDomain: frpDomain,
                  frpRemotePort: frpRemotePort ? Number(frpRemotePort) : null,
                  frpPublicHttps: checked
                })
              }}
              aria-label={t('remoteAccess.frpPublicHttps')}
            />
          </div>
        </div>
      )}

      {view.provider === 'sshReverse' && (
        <div className="space-y-3">
          <Field
            id="ssh-host"
            label={t('remoteAccess.sshHost')}
            hint={t('remoteAccess.sshHostHint')}
            value={sshHost}
            onChange={setSshHost}
            placeholder="vps.example.com"
          />
          <Field
            id="ssh-port"
            label={t('remoteAccess.sshPort')}
            hint={t('remoteAccess.sshPortHint')}
            value={sshPort}
            onChange={setSshPort}
            placeholder="22"
            inputMode="numeric"
          />
          <Field
            id="ssh-user"
            label={t('remoteAccess.sshUser')}
            hint={t('remoteAccess.sshUserHint')}
            value={sshUser}
            onChange={setSshUser}
            placeholder="ubuntu"
          />
          <Field
            id="ssh-remote-port"
            label={t('remoteAccess.sshRemotePort')}
            hint={t('remoteAccess.sshRemotePortHint')}
            value={sshRemotePort}
            onChange={setSshRemotePort}
            placeholder="18787"
            inputMode="numeric"
          />
          <Field
            id="ssh-public-hostname"
            label={t('remoteAccess.sshPublicHostname')}
            hint={t('remoteAccess.sshPublicHostnameHint')}
            value={sshPublicHostname}
            onChange={setSshPublicHostname}
            placeholder="termul.example.com"
          />
          <Field
            id="ssh-private-key"
            label={t('remoteAccess.sshPrivateKey')}
            hint={
              view.sshPrivateKeySet
                ? t('remoteAccess.tokenSet')
                : t('remoteAccess.sshPrivateKeyHint')
            }
            value={sshPrivateKey}
            onChange={setSshPrivateKey}
            type="password"
            autoComplete="off"
          />
          <div className="flex items-center justify-between gap-3">
            <div>
              <div className="text-sm text-foreground">{t('remoteAccess.sshPublicHttps')}</div>
              <p className="text-xs text-muted-foreground mt-0.5">
                {t('remoteAccess.sshPublicHttpsHint')}
              </p>
            </div>
            <Switch
              checked={view.sshPublicHttps}
              disabled={busy}
              onCheckedChange={(checked) => {
                setView((current) => ({ ...current, sshPublicHttps: checked }))
                void persist({
                  provider: 'sshReverse',
                  cloudflareNamedHostname: hostname,
                  frpServerAddr: frpAddr,
                  frpServerPort: frpPort ? Number(frpPort) : null,
                  frpCustomDomain: frpDomain,
                  frpRemotePort: frpRemotePort ? Number(frpRemotePort) : null,
                  sshHost,
                  sshPort: sshPort ? Number(sshPort) : null,
                  sshUser,
                  sshRemotePort: sshRemotePort ? Number(sshRemotePort) : null,
                  sshPublicHostname,
                  sshPublicHttps: checked
                })
              }}
              aria-label={t('remoteAccess.sshPublicHttps')}
            />
          </div>
        </div>
      )}

      {view.provider !== 'cloudflareQuick' && (
        <button
          type="button"
          disabled={busy}
          onClick={saveDetails}
          className="inline-flex items-center justify-center rounded-md border border-border bg-secondary px-3 py-1.5 text-xs hover:bg-secondary/80 disabled:opacity-50"
        >
          {t('remoteAccess.save')}
        </button>
      )}
    </div>
  )
}

function Field({
  id,
  label,
  hint,
  value,
  onChange,
  placeholder,
  type = 'text',
  inputMode,
  autoComplete
}: {
  id: string
  label: string
  hint: string
  value: string
  onChange: (value: string) => void
  placeholder?: string
  type?: string
  inputMode?: React.HTMLAttributes<HTMLInputElement>['inputMode']
  autoComplete?: string
}): React.JSX.Element {
  return (
    <div>
      <label className="block text-sm font-medium text-secondary-foreground mb-2" htmlFor={id}>
        {label}
      </label>
      <Input
        id={id}
        type={type}
        inputMode={inputMode}
        autoComplete={autoComplete}
        value={value}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
      />
      <p className="text-xs text-muted-foreground mt-1">{hint}</p>
    </div>
  )
}
