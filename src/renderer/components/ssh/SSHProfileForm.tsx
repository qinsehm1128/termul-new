import type { SSHAuthMethod, SSHProfile } from '@shared/types/ssh.types'
import { FolderOpen, X } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { useSshTranslation } from '@/hooks/use-ssh-translation'
import { dialogApi } from '@/lib/api'
import { useSSHActions } from '@/stores/ssh-store'

interface SSHProfileFormProps {
  profile: SSHProfile | null
  onClose: () => void
  onSaved: () => void
}

export function SSHProfileForm({
  profile,
  onClose,
  onSaved
}: SSHProfileFormProps): React.JSX.Element {
  const t = useSshTranslation()
  const { saveProfile } = useSSHActions()

  const [name, setName] = useState(profile?.name ?? '')
  const [host, setHost] = useState(profile?.host ?? '')
  const [port, setPort] = useState(profile?.port ?? 22)
  const [username, setUsername] = useState(profile?.username ?? '')
  const [authMethod, setAuthMethod] = useState<SSHAuthMethod>(profile?.authMethod ?? 'key')
  const [privateKeyPath, setPrivateKeyPath] = useState(profile?.privateKeyPath ?? '')
  // Security: never hydrate credentials from stored profile - require re-entry
  const [password, setPassword] = useState('')
  const [passphrase, setPassphrase] = useState('')
  const [saving, setSaving] = useState(false)

  const handleSelectKeyFile = async () => {
    try {
      const result = await dialogApi.selectFile({
        title: t('profile.selectPrivateKey'),
        filters: [{ name: t('profile.allFiles'), extensions: ['*'] }]
      })
      if (result.success) {
        setPrivateKeyPath(result.data)
      } else if (result.code !== 'CANCELLED') {
        toast.error(t('profile.selectFileFailed', { error: result.error }))
      }
    } catch (error) {
      toast.error(
        t('profile.fileDialogFailed', {
          error: error instanceof Error ? error.message : String(error)
        })
      )
    }
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()

    if (!name.trim() || !host.trim() || !username.trim()) {
      toast.error(t('profile.requiredFields'))
      return
    }

    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      toast.error(t('profile.invalidPort'))
      return
    }

    setSaving(true)
    try {
      const profileData: SSHProfile = {
        id: profile?.id ?? Date.now().toString(),
        name: name.trim(),
        host: host.trim(),
        port,
        username: username.trim(),
        authMethod,
        privateKeyPath: authMethod === 'key' ? privateKeyPath.trim() || undefined : undefined,
        // Only send password/passphrase if user entered a new value
        password: authMethod === 'password' && password ? password : undefined,
        passphrase: authMethod === 'key' && passphrase ? passphrase : undefined,
        portForwards: profile?.portForwards ?? [],
        tags: profile?.tags,
        lastConnected: profile?.lastConnected,
        importedFrom: profile?.importedFrom,
        hasStoredPassword: profile?.hasStoredPassword,
        hasStoredPassphrase: profile?.hasStoredPassphrase
      }

      const success = await saveProfile(profileData)
      if (success) {
        toast.success(profile ? t('profile.updated') : t('profile.created'))
        onSaved()
      } else {
        toast.error(t('profile.saveFailed'))
      }
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-[2px]">
      <div className="max-h-[80vh] w-[420px] overflow-y-auto rounded-md border border-border/80 bg-card shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]">
        {/* Header */}
        <div className="flex h-9 items-center justify-between border-b border-border/70 px-3">
          <h3 className="text-xs font-semibold tracking-[-0.01em] text-foreground">
            {profile ? t('profile.editTitle') : t('profile.newTitle')}
          </h3>
          <button
            onClick={onClose}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {/* Form */}
        <form onSubmit={handleSubmit}>
          <div className="space-y-3 p-4">
            {/* Name */}
            <div>
              <label className="text-xs font-medium text-muted-foreground">
                {t('profile.name')}
              </label>
              <input
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={t('profile.namePlaceholder')}
                className="mt-1 h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
              />
            </div>

            {/* Host + Port */}
            <div className="flex gap-3">
              <div className="flex-1">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('profile.host')}
                </label>
                <input
                  type="text"
                  value={host}
                  onChange={(e) => setHost(e.target.value)}
                  placeholder={t('profile.hostPlaceholder')}
                  className="mt-1 h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                />
              </div>
              <div className="w-20">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('profile.port')}
                </label>
                <input
                  type="number"
                  value={port}
                  onChange={(e) => setPort(Number(e.target.value))}
                  min={1}
                  max={65535}
                  className="mt-1 h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                />
              </div>
            </div>

            {/* Username */}
            <div>
              <label className="text-xs font-medium text-muted-foreground">
                {t('profile.username')}
              </label>
              <input
                type="text"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                placeholder={t('profile.usernamePlaceholder')}
                className="mt-1 h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
              />
            </div>

            {/* Auth Method */}
            <div>
              <label className="text-xs font-medium text-muted-foreground">
                {t('profile.authentication')}
              </label>
              <select
                value={authMethod}
                onChange={(e) => setAuthMethod(e.target.value as SSHAuthMethod)}
                className="mt-1 h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
              >
                <option value="key">{t('profile.auth.privateKey')}</option>
                <option value="password">{t('profile.auth.password')}</option>
                <option value="agent">{t('profile.auth.agent')}</option>
              </select>
            </div>

            {/* Private Key Path (conditional) */}
            {authMethod === 'key' && (
              <div>
                <label className="text-xs font-medium text-muted-foreground">
                  {t('profile.privateKeyPath')}
                </label>
                <div className="flex gap-2 mt-1">
                  <input
                    type="text"
                    value={privateKeyPath}
                    onChange={(e) => setPrivateKeyPath(e.target.value)}
                    placeholder="~/.ssh/id_rsa"
                    className="h-8 flex-1 rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                  />
                  <button
                    type="button"
                    onClick={handleSelectKeyFile}
                    className="inline-flex h-8 items-center rounded-md border border-border/80 bg-secondary/50 px-2 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    title={t('profile.browsePrivateKey')}
                  >
                    <FolderOpen className="h-4 w-4" />
                  </button>
                </div>
              </div>
            )}

            {/* Passphrase for key (conditional) */}
            {authMethod === 'key' && (
              <div>
                <label className="text-xs font-medium text-muted-foreground">
                  {t('profile.passphrase')}
                </label>
                <input
                  type="password"
                  value={passphrase}
                  onChange={(e) => setPassphrase(e.target.value)}
                  placeholder={
                    profile?.hasStoredPassphrase ? '••••••••' : t('profile.passphrasePlaceholder')
                  }
                  className="mt-1 h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                />
                {profile?.hasStoredPassphrase && (
                  <p className="mt-1 text-3xs text-muted-foreground">
                    {t('profile.passphraseStored')}
                  </p>
                )}
              </div>
            )}

            {/* Password (conditional) */}
            {authMethod === 'password' && (
              <div>
                <label className="text-xs font-medium text-muted-foreground">
                  {t('profile.password')}
                </label>
                <input
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder={
                    profile?.hasStoredPassword ? '••••••••' : t('profile.passwordPlaceholder')
                  }
                  className="mt-1 h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                />
                <p className="mt-1 text-3xs text-muted-foreground">
                  {profile?.hasStoredPassword
                    ? t('profile.passwordStored')
                    : t('profile.passwordStorageHint')}
                </p>
              </div>
            )}
          </div>
          {/* Actions */}
          <div className="flex h-10 items-center justify-end gap-2 border-t border-border/70 bg-secondary/20 px-4">
            <button
              type="button"
              onClick={onClose}
              className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            >
              {t('actions.cancel')}
            </button>
            <button
              type="submit"
              disabled={saving}
              className="inline-flex h-8 items-center rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
            >
              {saving ? t('actions.saving') : profile ? t('actions.update') : t('actions.create')}
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}
