import {
  AI_PROVIDER_KINDS,
  AI_PROVIDERS_REQUIRING_ENDPOINT,
  type AiChannel,
  type AiChannelsDocument,
  type AiModelProfile,
  type AiProviderKind,
  type AiRoute
} from '@shared/types/ai-channels.types'
import { Plus, Save, Sparkles, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { SettingsDivider } from '@/components/settings/SettingsLayout'
import {
  deleteAiChannelCredential,
  emptyAiChannelsDocument,
  getAiChannelCredentialStatus,
  loadAiChannels,
  saveAiChannels,
  setAiChannelCredential
} from '@/lib/ai-channels-persistence'
import { cn } from '@/lib/utils'

function newChannel(index: number): AiChannel {
  const id = `channel-${index}`
  return {
    id,
    displayName: `AI Channel ${index}`,
    provider: 'openAiCompatible',
    baseUrl: 'https://api.openai.com/v1',
    enabled: true,
    credentialRef: { kind: 'keyring', ref: `ai/channel/${id}`, hasCredential: false },
    modelIds: ['model']
  }
}

function newProfile(channel: AiChannel): AiModelProfile {
  return {
    id: `${channel.id}-profile`,
    channelId: channel.id,
    modelId: channel.modelIds[0] ?? 'model',
    enabled: true,
    capabilities: {
      descriptionAnalysis: true,
      fxRuntime: true,
      structuredOutput: true,
      toolCalling: true
    },
    maxOutputTokens: 8192,
    temperature: 0.2
  }
}

function ensureRoutes(document: AiChannelsDocument, profile: AiModelProfile): AiRoute[] {
  const routes = [...document.routes]
  for (const purpose of ['descriptionAnalysis', 'fxRuntime'] as const) {
    const existing = routes.find((route) => route.purpose === purpose)
    if (existing) {
      if (!existing.profileIds.includes(profile.id)) existing.profileIds.push(profile.id)
    } else {
      routes.push({ purpose, profileIds: [profile.id], maxAttempts: 1, timeoutMs: 30_000 })
    }
  }
  return routes
}

export default function AiChannelsPage(): React.JSX.Element {
  const { t } = useTranslation('ai')
  const [document, setDocument] = useState<AiChannelsDocument>(emptyAiChannelsDocument)
  const [credentialStates, setCredentialStates] = useState<Record<string, boolean>>({})
  const [secretDrafts, setSecretDrafts] = useState<Record<string, string>>({})
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    void loadAiChannels()
      .then((loaded) => {
        setDocument(loaded)
        return Promise.all(
          loaded.channels.map(async (channel) => {
            const status = await getAiChannelCredentialStatus(channel.id)
            return [channel.id, status.success ? status.data.hasCredential : false] as const
          })
        )
      })
      .then((states) => setCredentialStates(Object.fromEntries(states)))
      .catch(() => toast.error(t('page.failed')))
      .finally(() => setLoading(false))
  }, [t])

  const profilesByChannel = useMemo(() => {
    const grouped = new Map<string, AiModelProfile[]>()
    for (const profile of document.profiles) {
      const profiles = grouped.get(profile.channelId) ?? []
      profiles.push(profile)
      grouped.set(profile.channelId, profiles)
    }
    return grouped
  }, [document.profiles])

  const updateChannel = (id: string, patch: Partial<AiChannel>): void => {
    setDocument((current) => ({
      ...current,
      channels: current.channels.map((channel) =>
        channel.id === id ? { ...channel, ...patch } : channel
      )
    }))
  }

  const addChannel = (): void => {
    setDocument((current) => {
      const channel = newChannel(current.channels.length + 1)
      const profile = newProfile(channel)
      return {
        ...current,
        revision: current.revision + 1,
        channels: [...current.channels, channel],
        profiles: [...current.profiles, profile],
        routes: ensureRoutes(current, profile)
      }
    })
  }

  const removeChannel = (id: string): void => {
    setDocument((current) => ({
      ...current,
      revision: current.revision + 1,
      channels: current.channels.filter((channel) => channel.id !== id),
      profiles: current.profiles.filter((profile) => profile.channelId !== id),
      routes: current.routes
        .map((route) => ({
          ...route,
          profileIds: route.profileIds.filter((profileId) =>
            current.profiles.some((profile) => profile.id === profileId && profile.channelId !== id)
          )
        }))
        .filter((route) => route.profileIds.length > 0)
    }))
  }

  const save = async (): Promise<void> => {
    setSaving(true)
    try {
      await saveAiChannels({ ...document, revision: Math.max(1, document.revision + 1) })
      setDocument((current) => ({ ...current, revision: current.revision + 1 }))
      toast.success(t('page.saved'))
    } catch {
      toast.error(t('page.failed'))
    } finally {
      setSaving(false)
    }
  }

  const saveCredential = async (channel: AiChannel): Promise<void> => {
    const value = secretDrafts[channel.id]?.trim()
    if (!value) return
    try {
      await setAiChannelCredential(channel, value)
      setCredentialStates((current) => ({ ...current, [channel.id]: true }))
      setSecretDrafts((current) => ({ ...current, [channel.id]: '' }))
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('page.failed'))
    }
  }

  const removeCredential = async (channel: AiChannel): Promise<void> => {
    try {
      await deleteAiChannelCredential(channel.id)
      setCredentialStates((current) => ({ ...current, [channel.id]: false }))
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('page.failed'))
    }
  }

  if (loading) return <div className="p-6 text-sm text-muted-foreground">Loading…</div>

  return (
    <div className="flex h-full flex-col overflow-auto bg-background">
      <header className="border-b border-border px-6 py-4">
        <div className="flex items-center gap-2">
          <Sparkles size={16} className="text-primary" />
          <h1 className="text-lg font-medium text-foreground">{t('page.title')}</h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">{t('page.subtitle')}</p>
        <p className="mt-2 text-xs text-muted-foreground">{t('page.security')}</p>
      </header>
      <main className="mx-auto w-full max-w-4xl space-y-4 p-6">
        <SettingsDivider label={t('page.title')} />
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-medium text-foreground">{t('page.title')}</h2>
          <div className="flex gap-2">
            <button type="button" className="btn-secondary" onClick={addChannel}>
              <Plus size={14} /> {t('page.addChannel')}
            </button>
            <button
              type="button"
              className="btn-primary"
              disabled={saving}
              onClick={() => void save()}
            >
              <Save size={14} /> {t('page.save')}
            </button>
          </div>
        </div>
        {document.channels.length === 0 ? (
          <div className="rounded-lg border border-dashed border-border p-8 text-center text-sm text-muted-foreground">
            {t('page.noChannels')}
          </div>
        ) : null}
        {document.channels.map((channel) => {
          const profiles = profilesByChannel.get(channel.id) ?? []
          const requiresEndpoint = AI_PROVIDERS_REQUIRING_ENDPOINT.includes(channel.provider)
          return (
            <section
              key={channel.id}
              className="space-y-4 rounded-lg border border-border bg-card p-5"
            >
              <div className="flex items-start justify-between gap-4">
                <div className="grid flex-1 gap-3 sm:grid-cols-2">
                  <label className="grid gap-1 text-xs text-muted-foreground">
                    {t('page.channelId')}
                    <input className="input" value={channel.id} readOnly />
                  </label>
                  <label className="grid gap-1 text-xs text-muted-foreground">
                    {t('page.displayName')}
                    <input
                      className="input"
                      value={channel.displayName}
                      onChange={(event) =>
                        updateChannel(channel.id, { displayName: event.target.value })
                      }
                    />
                  </label>
                  <label className="grid gap-1 text-xs text-muted-foreground">
                    {t('page.provider')}
                    <select
                      className="input"
                      value={channel.provider}
                      onChange={(event) =>
                        updateChannel(channel.id, {
                          provider: event.target.value as AiProviderKind
                        })
                      }
                    >
                      {AI_PROVIDER_KINDS.map((provider) => (
                        <option key={provider} value={provider}>
                          {t(`providers.${provider}`)}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label className="flex items-center gap-2 pt-5 text-sm text-foreground">
                    <input
                      type="checkbox"
                      checked={channel.enabled}
                      onChange={(event) =>
                        updateChannel(channel.id, { enabled: event.target.checked })
                      }
                    />
                    {t('page.enabled')}
                  </label>
                  {requiresEndpoint ? (
                    <label className="grid gap-1 text-xs text-muted-foreground sm:col-span-2">
                      {t('page.endpoint')}
                      <input
                        className="input"
                        value={channel.baseUrl ?? ''}
                        onChange={(event) =>
                          updateChannel(channel.id, { baseUrl: event.target.value })
                        }
                      />
                    </label>
                  ) : null}
                </div>
                <button
                  type="button"
                  className="text-muted-foreground hover:text-destructive"
                  aria-label={t('page.delete')}
                  onClick={() => removeChannel(channel.id)}
                >
                  <Trash2 size={16} />
                </button>
              </div>
              <div className="border-t border-border pt-4">
                <h3 className="mb-3 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                  {t('page.profile')}
                </h3>
                {profiles.map((profile) => (
                  <div key={profile.id} className="grid gap-3 sm:grid-cols-2">
                    <label className="grid gap-1 text-xs text-muted-foreground">
                      {t('page.model')}
                      <input
                        className="input"
                        value={profile.modelId}
                        onChange={(event) =>
                          setDocument((current) => ({
                            ...current,
                            profiles: current.profiles.map((item) =>
                              item.id === profile.id
                                ? { ...item, modelId: event.target.value }
                                : item
                            )
                          }))
                        }
                      />
                    </label>
                    <div className="grid gap-2 text-xs text-muted-foreground">
                      {t('page.credential')}:{' '}
                      {credentialStates[channel.id]
                        ? t('page.credentialPresent')
                        : t('page.credentialMissing')}
                      <div className="flex gap-2">
                        <input
                          className="input flex-1"
                          type="password"
                          autoComplete="new-password"
                          value={secretDrafts[channel.id] ?? ''}
                          placeholder={t('page.secretPlaceholder')}
                          onChange={(event) =>
                            setSecretDrafts((current) => ({
                              ...current,
                              [channel.id]: event.target.value
                            }))
                          }
                        />
                        <button
                          type="button"
                          className="btn-secondary"
                          onClick={() => void saveCredential(channel)}
                        >
                          {credentialStates[channel.id]
                            ? t('page.replaceCredential')
                            : t('page.setCredential')}
                        </button>
                        {credentialStates[channel.id] ? (
                          <button
                            type="button"
                            className="btn-secondary"
                            onClick={() => void removeCredential(channel)}
                          >
                            {t('page.removeCredential')}
                          </button>
                        ) : null}
                      </div>
                    </div>
                  </div>
                ))}
              </div>
              <div
                className={cn(
                  'border-t border-border pt-4 text-xs text-muted-foreground',
                  !profiles.length && 'hidden'
                )}
              >
                {t('page.routes')}:{' '}
                {document.routes
                  .map((route) => `${t(`page.${route.purpose}`)} (${route.profileIds.length})`)
                  .join(' · ')}
              </div>
            </section>
          )
        })}
      </main>
    </div>
  )
}
