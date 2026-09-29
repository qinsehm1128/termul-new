/**
 * MCP page, "Server descriptions": the one line an agent reads to choose a
 * server. A model from the AI channels page can write them; any line can be
 * edited or reset to what the server says about itself.
 */

import { Loader2, Pencil, Sparkles } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { type McpServerSummary, mcpGatewayApi } from '@/lib/mcp-gateway-api'
import { useMcpGatewayStore } from '@/stores/mcp-gateway-store'

/** The language the model writes in, from the UI language. */
export function descriptionLanguage(uiLanguage: string): string {
  return uiLanguage.toLowerCase().startsWith('zh') ? 'Chinese' : 'English'
}

export function McpDescriptionsPanel(): React.JSX.Element {
  const { t, i18n } = useTranslation('mcp')
  const upstreams = useMcpGatewayStore((state) => state.view?.status?.upstreams)
  const [servers, setServers] = useState<McpServerSummary[]>([])
  const [stored, setStored] = useState<Record<string, string>>({})
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null)
  const [busy, setBusy] = useState<string | null>(null)

  const load = useCallback(async () => {
    const [listed, descriptions] = await Promise.all([
      mcpGatewayApi.servers(),
      mcpGatewayApi.descriptions()
    ])
    if (listed.success) setServers(listed.data.servers.filter((server) => !server.builtIn))
    if (descriptions.success) setStored(descriptions.data)
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const idOf = (name: string): string | undefined =>
    upstreams?.find((upstream) => upstream.name === name)?.id

  const saveDescriptions = async (next: Record<string, string>): Promise<void> => {
    setBusy('save')
    const result = await mcpGatewayApi.putDescriptions(next)
    setBusy(null)
    if (!result.success) {
      toast.error(result.error ?? t('descriptions.failed'))
      return
    }
    setEditing(null)
    // The gateway reloads the file; read back what agents now see.
    await load()
  }

  const describe = async (names?: string[]): Promise<void> => {
    setBusy(names?.[0] ?? 'all')
    const result = await mcpGatewayApi.describe(descriptionLanguage(i18n.language), names)
    setBusy(null)
    if (!result.success) {
      toast.error(
        result.code === 'AI_NOT_CONFIGURED'
          ? t('descriptions.notConfigured')
          : (result.error ?? t('descriptions.failed'))
      )
      return
    }
    const failed = result.data.filter((item) => item.error)
    if (failed.length > 0) {
      toast.error(
        t('descriptions.someFailed', {
          names: failed.map((item) => `${item.name}: ${item.error}`).join('; ')
        })
      )
    }
    await load()
  }

  return (
    <section className="space-y-3 rounded-lg border border-border bg-secondary/20 p-4">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium text-foreground">{t('descriptions.title')}</p>
          <p className="text-xs text-muted-foreground">{t('descriptions.description')}</p>
        </div>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={busy !== null || servers.length === 0}
          onClick={() => void describe()}
        >
          {busy === 'all' ? <Loader2 className="animate-spin" /> : <Sparkles />}
          {t('descriptions.generateAll')}
        </Button>
      </div>
      {servers.length === 0 ? (
        <p className="text-xs text-muted-foreground">{t('descriptions.empty')}</p>
      ) : (
        <ul className="divide-y divide-border rounded-md border border-border">
          {servers.map((server) => {
            const id = idOf(server.name)
            const custom = id !== undefined && stored[id] !== undefined
            const isEditing = editing !== null && editing.id === id
            return (
              <li key={server.name} className="space-y-1 px-3 py-2">
                <div className="flex items-center gap-2">
                  <span className="font-mono text-xs font-medium text-foreground">
                    {server.name}
                  </span>
                  {custom ? (
                    <span className="text-3xs text-muted-foreground">
                      {t('descriptions.custom')}
                    </span>
                  ) : null}
                  <div className="ml-auto flex items-center gap-1">
                    {busy === server.name ? (
                      <Loader2 className="size-3.5 animate-spin text-muted-foreground" />
                    ) : null}
                    <Button
                      type="button"
                      size="icon-xs"
                      variant="ghost"
                      disabled={busy !== null || id === undefined}
                      aria-label={t('descriptions.generate', { name: server.name })}
                      onClick={() => void describe([server.name])}
                    >
                      <Sparkles />
                    </Button>
                    <Button
                      type="button"
                      size="icon-xs"
                      variant="ghost"
                      disabled={busy !== null || id === undefined}
                      aria-label={t('descriptions.edit', { name: server.name })}
                      onClick={() => id && setEditing({ id, text: server.description })}
                    >
                      <Pencil />
                    </Button>
                  </div>
                </div>
                {isEditing && editing ? (
                  <form
                    className="flex gap-2"
                    onSubmit={(event) => {
                      event.preventDefault()
                      void saveDescriptions({ ...stored, [editing.id]: editing.text })
                    }}
                  >
                    <Input
                      aria-label={t('descriptions.editLabel', { name: server.name })}
                      value={editing.text}
                      maxLength={240}
                      onChange={(event) => setEditing({ ...editing, text: event.target.value })}
                    />
                    <Button type="submit" size="sm" disabled={busy !== null}>
                      {t('descriptions.save')}
                    </Button>
                    {custom ? (
                      <Button
                        type="button"
                        size="sm"
                        variant="ghost"
                        disabled={busy !== null}
                        onClick={() => {
                          const { [editing.id]: _removed, ...rest } = stored
                          void saveDescriptions(rest)
                        }}
                      >
                        {t('descriptions.reset')}
                      </Button>
                    ) : null}
                  </form>
                ) : (
                  <p className="text-xs text-muted-foreground">
                    {server.description || t('descriptions.none')}
                  </p>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </section>
  )
}
