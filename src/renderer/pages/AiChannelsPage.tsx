import { BrainCircuit, Download, Loader2, Plus, Save, Trash2, X, Zap } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  AI_APIS,
  AI_CHANNEL_PRESETS,
  type AiApi,
  type AiChannel,
  type AiChannelPresetId,
  type AiChannelsDocument,
  aiChannelsApi,
  emptyAiChannels,
  newChannelId
} from '@/lib/ai-channels-api'
import { isTauriContext } from '@/lib/tauri-runtime'

const SELECT_CLASS =
  'h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2 text-sm text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring/35'

function ChannelCard({
  channel,
  hasKey,
  onChange,
  onRemove,
  onKeyChange
}: {
  channel: AiChannel
  hasKey: boolean
  onChange: (next: AiChannel) => void
  onRemove: () => void
  onKeyChange: (stored: boolean) => void
}): React.JSX.Element {
  const { t } = useTranslation('ai')
  const [keyDraft, setKeyDraft] = useState('')
  const [modelDraft, setModelDraft] = useState('')
  const [discovered, setDiscovered] = useState<string[] | null>(null)
  const [testModel, setTestModel] = useState(channel.models[0] ?? '')
  const [busy, setBusy] = useState<'key' | 'models' | 'test' | null>(null)
  const [testResult, setTestResult] = useState<string | null>(null)

  const update = (patch: Partial<AiChannel>): void => onChange({ ...channel, ...patch })
  const addModels = (models: string[]): void => {
    const next = [...channel.models]
    for (const model of models.map((value) => value.trim()).filter(Boolean)) {
      if (!next.includes(model)) next.push(model)
    }
    update({ models: next })
    if (!testModel && next[0]) setTestModel(next[0])
  }

  const storeKey = async (key: string | null): Promise<void> => {
    setBusy('key')
    const result = await aiChannelsApi.setKey(channel.id, key)
    setBusy(null)
    if (!result.success) {
      toast.error(result.error ?? t('page.failed'))
      return
    }
    setKeyDraft('')
    onKeyChange(key !== null)
  }

  const discover = async (): Promise<void> => {
    setBusy('models')
    const result = await aiChannelsApi.models(channel)
    setBusy(null)
    if (!result.success) {
      toast.error(result.error ?? t('page.failed'))
      return
    }
    setDiscovered(result.data)
  }

  const test = async (): Promise<void> => {
    setBusy('test')
    setTestResult(null)
    const result = await aiChannelsApi.test(channel, testModel)
    setBusy(null)
    setTestResult(
      result.success
        ? t('page.testOk', { millis: result.data.millis, reply: result.data.reply })
        : t('page.testFailed', { error: result.error ?? '' })
    )
  }

  return (
    <section
      aria-label={channel.name || t('page.untitled')}
      className="space-y-4 rounded-lg border border-border bg-card p-4"
    >
      <div className="flex items-center gap-3">
        <Switch
          checked={channel.enabled}
          aria-label={t('page.enabled')}
          onCheckedChange={(enabled) => update({ enabled })}
        />
        <Input
          aria-label={t('page.name')}
          className="h-8 max-w-xs font-medium"
          value={channel.name}
          placeholder={t('page.untitled')}
          onChange={(event) => update({ name: event.target.value })}
        />
        <Button
          type="button"
          size="icon-sm"
          variant="ghost"
          className="ml-auto"
          aria-label={t('page.remove', { name: channel.name })}
          onClick={onRemove}
        >
          <Trash2 />
        </Button>
      </div>

      <div className="grid gap-3 sm:grid-cols-[14rem_1fr]">
        <label className="grid gap-1 text-xs text-muted-foreground">
          {t('page.api')}
          <select
            className={SELECT_CLASS}
            value={channel.api}
            onChange={(event) => update({ api: event.target.value as AiApi })}
          >
            {AI_APIS.map((api) => (
              <option key={api} value={api}>
                {t(`apis.${api}`)}
              </option>
            ))}
          </select>
        </label>
        <label
          htmlFor={`ai-base-url-${channel.id}`}
          className="grid gap-1 text-xs text-muted-foreground"
        >
          {t('page.baseUrl')}
          <Input
            id={`ai-base-url-${channel.id}`}
            className="font-mono"
            value={channel.baseUrl}
            placeholder={t(`baseUrlHint.${channel.api}`)}
            onChange={(event) => update({ baseUrl: event.target.value })}
          />
        </label>
      </div>

      <div className="grid gap-1 text-xs text-muted-foreground">
        <span>
          {t('page.apiKey')} ·{' '}
          <span className={hasKey ? 'text-emerald-600 dark:text-emerald-400' : ''}>
            {hasKey ? t('page.keyStored') : t('page.keyMissing')}
          </span>
        </span>
        <div className="flex gap-2">
          <Input
            type="password"
            autoComplete="off"
            aria-label={t('page.apiKey')}
            value={keyDraft}
            placeholder={hasKey ? t('page.keyReplacePlaceholder') : t('page.keyPlaceholder')}
            onChange={(event) => setKeyDraft(event.target.value)}
          />
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={keyDraft.trim() === '' || busy !== null}
            onClick={() => void storeKey(keyDraft)}
          >
            {t('page.saveKey')}
          </Button>
          {hasKey ? (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              disabled={busy !== null}
              onClick={() => void storeKey(null)}
            >
              {t('page.removeKey')}
            </Button>
          ) : null}
        </div>
      </div>

      <div className="space-y-2">
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>{t('page.models')}</span>
          <Button
            type="button"
            size="xs"
            variant="ghost"
            disabled={!hasKey || channel.baseUrl.trim() === '' || busy !== null}
            onClick={() => void discover()}
          >
            {busy === 'models' ? <Loader2 className="animate-spin" /> : <Download />}
            {t('page.discover')}
          </Button>
        </div>
        <ul className="flex flex-wrap gap-1.5">
          {channel.models.map((model) => (
            <li
              key={model}
              className="inline-flex items-center gap-1 rounded bg-secondary px-2 py-0.5 font-mono text-2xs"
            >
              {model}
              <button
                type="button"
                aria-label={t('page.removeModel', { model })}
                className="text-muted-foreground hover:text-foreground"
                onClick={() => update({ models: channel.models.filter((item) => item !== model) })}
              >
                <X className="size-3" />
              </button>
            </li>
          ))}
        </ul>
        {discovered ? (
          <div className="space-y-1 rounded-md border border-border p-2">
            <p className="text-2xs text-muted-foreground">
              {t('page.discovered', { count: discovered.length })}
            </p>
            <div className="flex max-h-40 flex-wrap gap-1 overflow-auto">
              {discovered
                .filter((model) => !channel.models.includes(model))
                .map((model) => (
                  <button
                    key={model}
                    type="button"
                    className="rounded border border-border px-1.5 py-0.5 font-mono text-2xs hover:bg-secondary"
                    onClick={() => addModels([model])}
                  >
                    + {model}
                  </button>
                ))}
            </div>
          </div>
        ) : null}
        <form
          className="flex gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            addModels(modelDraft.split(','))
            setModelDraft('')
          }}
        >
          <Input
            aria-label={t('page.addModel')}
            className="font-mono"
            value={modelDraft}
            placeholder={t('page.addModelPlaceholder')}
            onChange={(event) => setModelDraft(event.target.value)}
          />
          <Button type="submit" size="sm" variant="outline" disabled={modelDraft.trim() === ''}>
            <Plus />
            {t('page.addModel')}
          </Button>
        </form>
      </div>

      <div className="flex flex-wrap items-center gap-2 border-t border-border pt-3">
        <select
          aria-label={t('page.testModel')}
          className={`${SELECT_CLASS} max-w-xs`}
          value={testModel}
          onChange={(event) => setTestModel(event.target.value)}
        >
          {channel.models.map((model) => (
            <option key={model} value={model}>
              {model}
            </option>
          ))}
        </select>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={!hasKey || !testModel || busy !== null}
          onClick={() => void test()}
        >
          {busy === 'test' ? <Loader2 className="animate-spin" /> : <Zap />}
          {t('page.test')}
        </Button>
        {testResult ? (
          <span className="min-w-0 truncate text-xs text-muted-foreground" title={testResult}>
            {testResult}
          </span>
        ) : null}
      </div>
    </section>
  )
}

export default function AiChannelsPage(): React.JSX.Element {
  const { t } = useTranslation('ai')
  const [document, setDocument] = useState<AiChannelsDocument>(emptyAiChannels)
  const [keys, setKeys] = useState<Record<string, boolean>>({})
  const [dirty, setDirty] = useState(false)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    void aiChannelsApi.load().then((result) => {
      if (result.success) {
        setDocument(result.data.document)
        setKeys(result.data.keys)
      } else if (result.code !== 'DESKTOP_ONLY') {
        toast.error(result.error ?? t('page.failed'))
      }
      setLoading(false)
    })
  }, [t])

  const change = (next: AiChannelsDocument): void => {
    setDocument(next)
    setDirty(true)
  }

  const addChannel = (presetId: AiChannelPresetId): void => {
    const preset = AI_CHANNEL_PRESETS.find((item) => item.id === presetId)
    if (!preset) return
    change({
      ...document,
      channels: [
        ...document.channels,
        {
          id: newChannelId(),
          name: preset.name,
          api: preset.api,
          baseUrl: preset.baseUrl,
          enabled: true,
          models: []
        }
      ]
    })
  }

  const save = async (): Promise<void> => {
    setSaving(true)
    const result = await aiChannelsApi.save(document)
    setSaving(false)
    if (result.success) {
      setDirty(false)
      toast.success(t('page.saved'))
    } else {
      toast.error(result.error ?? t('page.failed'))
    }
  }

  if (!isTauriContext()) {
    return <div className="p-6 text-sm text-muted-foreground">{t('page.desktopOnly')}</div>
  }
  if (loading) return <div className="p-6 text-sm text-muted-foreground">{t('page.loading')}</div>

  const summary = document.purposes.mcpSummary
  const summaryChannel = document.channels.find((channel) => channel.id === summary?.channelId)

  return (
    <div className="flex h-full flex-col overflow-auto bg-background">
      <header className="border-b border-border px-6 py-4">
        <div className="flex items-center gap-2">
          <BrainCircuit size={16} className="text-primary" />
          <h1 className="text-lg font-medium text-foreground">{t('page.title')}</h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">{t('page.subtitle')}</p>
      </header>
      <main className="mx-auto w-full max-w-4xl space-y-4 p-6">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-medium text-foreground">{t('page.channels')}</span>
          <div className="ml-auto flex flex-wrap gap-2">
            {AI_CHANNEL_PRESETS.map((preset) => (
              <Button
                key={preset.id}
                type="button"
                size="sm"
                variant="outline"
                onClick={() => addChannel(preset.id)}
              >
                <Plus />
                {t(`presets.${preset.id}`)}
              </Button>
            ))}
            <Button type="button" size="sm" disabled={!dirty || saving} onClick={() => void save()}>
              {saving ? <Loader2 className="animate-spin" /> : <Save />}
              {t('page.save')}
            </Button>
          </div>
        </div>

        {document.channels.length === 0 ? (
          <div className="rounded-lg border border-dashed border-border p-8 text-center text-sm text-muted-foreground">
            {t('page.noChannels')}
          </div>
        ) : null}
        {document.channels.map((channel) => (
          <ChannelCard
            key={channel.id}
            channel={channel}
            hasKey={keys[channel.id] === true}
            onKeyChange={(stored) => setKeys((current) => ({ ...current, [channel.id]: stored }))}
            onChange={(next) =>
              change({
                ...document,
                channels: document.channels.map((item) => (item.id === next.id ? next : item))
              })
            }
            onRemove={() =>
              change({
                ...document,
                channels: document.channels.filter((item) => item.id !== channel.id),
                purposes:
                  summary?.channelId === channel.id
                    ? { ...document.purposes, mcpSummary: undefined }
                    : document.purposes
              })
            }
          />
        ))}

        <section className="space-y-3 rounded-lg border border-border bg-secondary/20 p-4">
          <div>
            <p className="text-sm font-medium text-foreground">{t('page.purposes')}</p>
            <p className="text-xs text-muted-foreground">{t('page.purposesDescription')}</p>
          </div>
          <div className="grid gap-2 sm:grid-cols-[12rem_1fr_1fr] sm:items-center">
            <span className="text-sm text-foreground">{t('purposes.mcpSummary')}</span>
            <select
              aria-label={t('page.purposeChannel')}
              className={SELECT_CLASS}
              value={summary?.channelId ?? ''}
              onChange={(event) => {
                const channel = document.channels.find((item) => item.id === event.target.value)
                change({
                  ...document,
                  purposes: {
                    ...document.purposes,
                    mcpSummary: channel
                      ? { channelId: channel.id, model: channel.models[0] ?? '' }
                      : undefined
                  }
                })
              }}
            >
              <option value="">{t('page.purposeNone')}</option>
              {document.channels.map((channel) => (
                <option key={channel.id} value={channel.id}>
                  {channel.name || t('page.untitled')}
                </option>
              ))}
            </select>
            <select
              aria-label={t('page.purposeModel')}
              className={SELECT_CLASS}
              value={summary?.model ?? ''}
              disabled={!summaryChannel}
              onChange={(event) =>
                summary &&
                change({
                  ...document,
                  purposes: {
                    ...document.purposes,
                    mcpSummary: { ...summary, model: event.target.value }
                  }
                })
              }
            >
              {(summaryChannel?.models ?? []).map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </div>
        </section>
        <p className="text-xs text-muted-foreground">{t('page.security')}</p>
      </main>
    </div>
  )
}
