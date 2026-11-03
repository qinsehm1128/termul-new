import {
  type ConfigSnapshot,
  type ConfigValue,
  type ConfigValues,
  validateConfigValues
} from '@shared/types/plugin.types'
import { useEffect, useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { logFrontendError } from '@/lib/log-api'
import { pluginApi } from './plugin-api'

type Draft = Record<string, ConfigValue | undefined>
type FormError = 'settings.conflict' | 'settings.invalid' | 'errors.generic' | null

function toDraft(snapshot: ConfigSnapshot): Draft {
  return Object.fromEntries(
    Object.entries(snapshot.values).map(([key, value]) => [
      key,
      snapshot.schema.properties[key].type === 'number' && !snapshot.schema.properties[key].enum
        ? String(value)
        : value
    ])
  )
}

/** A keyed editor prevents another plugin from inheriting a draft or pending request. */
export function PluginSettingsForm({ pluginId }: { pluginId: string }): React.JSX.Element {
  return <SettingsEditor key={pluginId} pluginId={pluginId} />
}

function SettingsEditor({ pluginId }: { pluginId: string }): React.JSX.Element {
  const { t } = useTranslation('plugins')
  const id = useId()
  const [snapshot, setSnapshot] = useState<ConfigSnapshot | null>(null)
  const [draft, setDraft] = useState<Draft>({})
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<FormError>(null)
  const [reload, setReload] = useState(0)
  const epoch = useRef(0)
  const busy = useRef(false)

  // biome-ignore lint/correctness/useExhaustiveDependencies: reload is an explicit user-requested refetch token.
  useEffect(() => {
    const request = ++epoch.current
    setLoading(true)
    void (async () => {
      try {
        const result = await pluginApi.getConfig({ id: pluginId })
        if (epoch.current !== request) return
        if (!result.success) {
          setError('errors.generic')
          return
        }
        setSnapshot(result.data)
        setDraft(toDraft(result.data))
        setError(null)
      } catch {
        if (epoch.current !== request) return
        setError('errors.generic')
        void logFrontendError({
          level: 'warn',
          source: 'PluginSettingsForm',
          message: 'Configuration load failed'
        })
      } finally {
        if (epoch.current === request) setLoading(false)
      }
    })()
    return () => {
      ++epoch.current
    }
  }, [pluginId, reload])

  function edit(key: string, value: ConfigValue | undefined): void {
    setDraft((current) => ({ ...current, [key]: value }))
    if (error !== 'settings.conflict') setError(null)
  }

  async function save(defaults = false): Promise<void> {
    if (!snapshot || loading || busy.current || error === 'settings.conflict') return
    let values: ConfigValues
    try {
      const input = Object.fromEntries(
        Object.entries(snapshot.schema.properties).map(([key, property]) => {
          let value = defaults ? property.default : draft[key]
          if (!defaults && property.type === 'number' && !property.enum) {
            // Empty/partial/non-decimal input must not silently coerce to zero.
            if (
              typeof value !== 'string' ||
              !/^[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?$/i.test(value)
            ) {
              throw new TypeError('Invalid number')
            }
            value = Number(value)
          }
          return [key, value]
        })
      )
      values = validateConfigValues(snapshot.schema, input, true)
    } catch {
      setError('settings.invalid')
      return
    }
    const request = epoch.current
    busy.current = true
    setSaving(true)
    setError(null)
    try {
      const result = await pluginApi.setConfig({
        id: pluginId,
        expectedRevision: snapshot.revision,
        values
      })
      if (epoch.current !== request) return
      if (!result.success) {
        setError(result.code === 'PLUGIN_CONFIG_CONFLICT' ? 'settings.conflict' : 'errors.generic')
        return
      }
      setSnapshot(result.data)
      setDraft(toDraft(result.data))
    } catch {
      if (epoch.current !== request) return
      setError('errors.generic')
      void logFrontendError({
        level: 'warn',
        source: 'PluginSettingsForm',
        message: 'Configuration save failed'
      })
    } finally {
      if (epoch.current === request) {
        busy.current = false
        setSaving(false)
      }
    }
  }

  const disabled = loading || saving
  const empty = snapshot && Object.keys(snapshot.schema.properties).length === 0

  return (
    <form
      className="space-y-4 text-foreground"
      aria-label={t('settings.title')}
      aria-busy={disabled}
      noValidate
      onSubmit={(event) => {
        event.preventDefault()
        void save()
      }}
    >
      <h2 className="text-sm font-medium">{snapshot?.schema.title ?? t('settings.title')}</h2>
      {loading && <p role="status">{t('view.loading')}</p>}
      {error && (
        <div className="space-y-2">
          <p role="alert" className="text-sm text-destructive">
            {t(error)}
          </p>
          <Button
            type="button"
            variant="outline"
            disabled={disabled}
            onClick={() => setReload((current) => current + 1)}
          >
            {t('settings.reload')}
          </Button>
        </div>
      )}
      {empty && <p className="text-sm text-muted-foreground">{t('settings.empty')}</p>}
      {snapshot &&
        Object.entries(snapshot.schema.properties).map(([key, property]) => {
          const fieldId = `${id}-${key}`
          const descriptionId = `${fieldId}-description`
          const value = draft[key]
          return (
            <div key={key} className="space-y-1.5">
              <label htmlFor={fieldId} className="text-sm font-medium">
                {property.title}
              </label>
              <p id={descriptionId} className="text-xs text-muted-foreground">
                {property.description}
              </p>
              {property.enum ? (
                <Select
                  value={String(property.enum.findIndex((option) => option === value))}
                  disabled={disabled}
                  onValueChange={(index) =>
                    edit(
                      key,
                      property.enum?.find((_, i) => String(i) === index)
                    )
                  }
                >
                  <SelectTrigger id={fieldId} aria-describedby={descriptionId}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {property.enum.map((option, index) => (
                      <SelectItem key={String(index)} value={String(index)}>
                        {property.enumLabels?.[index] ?? String(option)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : property.type === 'boolean' ? (
                <Switch
                  id={fieldId}
                  aria-describedby={descriptionId}
                  checked={value === true}
                  disabled={disabled}
                  onCheckedChange={(checked) => edit(key, checked)}
                />
              ) : (
                <Input
                  id={fieldId}
                  aria-describedby={descriptionId}
                  type={property.type === 'number' ? 'number' : 'text'}
                  step={property.type === 'number' ? 'any' : undefined}
                  min={property.minimum}
                  max={property.maximum}
                  value={String(value ?? '')}
                  disabled={disabled}
                  onChange={(event) => edit(key, event.target.value)}
                />
              )}
              <p className="text-xs text-muted-foreground">
                {t('settings.default')}: <code>{JSON.stringify(property.default)}</code>
              </p>
            </div>
          )
        })}
      {snapshot && !empty && (
        <div className="flex flex-wrap gap-2">
          <Button type="submit" disabled={disabled || error === 'settings.conflict'}>
            {t(saving ? 'settings.saving' : 'settings.save')}
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={disabled || error === 'settings.conflict'}
            onClick={() => void save(true)}
          >
            {t('settings.reset')}
          </Button>
        </div>
      )}
    </form>
  )
}
