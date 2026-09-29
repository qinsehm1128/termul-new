/**
 * MCP page, "Server descriptions" → "Generation settings": the prompt and
 * the limits the model writes descriptions under.
 */

import { ChevronDown, Loader2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { type McpDescribeSettings, mcpGatewayApi } from '@/lib/mcp-gateway-api'

type NumberField = 'maxChars' | 'toolDescriptionChars'

const NUMBER_FIELDS: readonly NumberField[] = ['maxChars', 'toolDescriptionChars']

interface Draft {
  prompt: string
  maxChars: string
  toolDescriptionChars: string
}

function toDraft(settings: McpDescribeSettings): Draft {
  return {
    prompt: settings.prompt,
    maxChars: String(settings.maxChars),
    toolDescriptionChars: String(settings.toolDescriptionChars)
  }
}

function fromDraft(draft: Draft): McpDescribeSettings {
  return {
    prompt: draft.prompt,
    maxChars: Number(draft.maxChars),
    toolDescriptionChars: Number(draft.toolDescriptionChars)
  }
}

export function McpDescribeSettingsForm(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const [draft, setDraft] = useState<Draft | null>(null)
  const [defaults, setDefaults] = useState<McpDescribeSettings | null>(null)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    void mcpGatewayApi.describeSettings().then((result) => {
      if (!result.success) {
        toast.error(result.error ?? t('describeSettings.failed'))
        return
      }
      setDraft(toDraft(result.data.settings))
      setDefaults(result.data.defaults)
    })
  }, [t])

  const save = async (next: Draft): Promise<void> => {
    setSaving(true)
    const result = await mcpGatewayApi.putDescribeSettings(fromDraft(next))
    setSaving(false)
    if (!result.success) {
      toast.error(result.error ?? t('describeSettings.failed'))
      return
    }
    setDraft(next)
    toast.success(t('describeSettings.saved'))
  }

  return (
    <Collapsible>
      <CollapsibleTrigger className="group inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
        <ChevronDown className="size-3.5 transition-transform group-data-[state=open]:rotate-180" />
        {t('describeSettings.title')}
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-2">
        {draft ? (
          <form
            className="space-y-3"
            onSubmit={(event) => {
              event.preventDefault()
              void save(draft)
            }}
          >
            <div className="space-y-1">
              <label htmlFor="mcp-describe-prompt" className="text-xs text-muted-foreground">
                {t('describeSettings.prompt')}
              </label>
              <Textarea
                id="mcp-describe-prompt"
                rows={6}
                className="font-mono text-xs"
                value={draft.prompt}
                onChange={(event) => setDraft({ ...draft, prompt: event.target.value })}
              />
              <p className="text-2xs text-muted-foreground">{t('describeSettings.promptHint')}</p>
            </div>
            <div className="grid grid-cols-2 gap-3">
              {NUMBER_FIELDS.map((field) => (
                <div key={field} className="space-y-1">
                  <label
                    htmlFor={`mcp-describe-${field}`}
                    className="text-xs text-muted-foreground"
                  >
                    {t(`describeSettings.${field}`)}
                  </label>
                  <Input
                    id={`mcp-describe-${field}`}
                    inputMode="numeric"
                    className="h-7 font-mono"
                    value={draft[field]}
                    onChange={(event) =>
                      setDraft({ ...draft, [field]: event.target.value.replace(/[^0-9]/g, '') })
                    }
                  />
                  <p className="text-2xs text-muted-foreground">
                    {t(`describeSettings.${field}Hint`)}
                  </p>
                </div>
              ))}
            </div>
            <div className="flex gap-2">
              <Button type="submit" size="sm" disabled={saving}>
                {saving ? <Loader2 className="animate-spin" /> : null}
                {t('describeSettings.save')}
              </Button>
              {defaults ? (
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  disabled={saving}
                  onClick={() => void save(toDraft(defaults))}
                >
                  {t('describeSettings.reset')}
                </Button>
              ) : null}
            </div>
          </form>
        ) : null}
      </CollapsibleContent>
    </Collapsible>
  )
}
