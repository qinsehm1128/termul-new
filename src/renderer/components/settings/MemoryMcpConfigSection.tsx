/**
 * "Memory MCP" export card (Settings → MCP Servers).
 *
 * The host's memory-index MCP server is this executable itself, and its state
 * root is something only the host knows. This card hands out the universal
 * invocation: one server that lists every indexed project and lets the client
 * choose per query, exported in either external-client shape — a Claude-style
 * `mcpServers` JSON block or a Codex `config.toml` `[mcp_servers.*]` table.
 *
 * Browser mode disables the card: the invocation names the host's executable
 * and state root, which a browser client can neither run nor see.
 */

import { Copy, Loader2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { toast } from '@/hooks/use-toast'
import { clipboardApi, memoryIndexApi } from '@/lib/api'
import { buildMcpClientConfig, type McpClientConfigFormat } from '@/lib/mcp-client-config'
import { isTauriContext } from '@/lib/tauri-runtime'
import { cn } from '@/lib/utils'

const FORMATS = [
  { id: 'json', labelKey: 'memory.formatJson' },
  { id: 'toml', labelKey: 'memory.formatToml' }
] as const

export function MemoryMcpConfigSection(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const [format, setFormat] = useState<McpClientConfigFormat>('json')
  const [isFetching, setIsFetching] = useState(false)
  const [preview, setPreview] = useState<string | null>(null)

  async function generateConfig(): Promise<string | null> {
    if (!isTauriContext()) {
      toast({ title: t('memory.unavailable'), variant: 'destructive' })
      return null
    }
    setIsFetching(true)
    try {
      const invocation = await memoryIndexApi.universalMcpInvocation()
      if (!invocation) {
        toast({ title: t('memory.unavailable'), variant: 'destructive' })
        return null
      }
      const config = buildMcpClientConfig(invocation, format)
      setPreview(config)
      return config
    } catch (error) {
      toast({
        title: t('memory.copyFailed'),
        description: error instanceof Error ? error.message : undefined,
        variant: 'destructive'
      })
      return null
    } finally {
      setIsFetching(false)
    }
  }

  async function handleCopy(): Promise<void> {
    const config = (await generateConfig()) ?? preview
    if (!config) return
    await clipboardApi.writeText(config)
    toast({ title: t('memory.copied') })
  }

  return (
    <div className="space-y-4 rounded-lg border border-border bg-secondary/20 p-4">
      <div>
        <p className="text-sm font-medium text-foreground">{t('memory.title')}</p>
        <p className="text-xs text-muted-foreground">{t('memory.description')}</p>
      </div>

      <div>
        <span className="mb-1.5 block text-xs font-medium text-muted-foreground">
          {t('memory.format')}
        </span>
        <div className="inline-flex h-8 items-center rounded-md border border-input/80 bg-secondary/35 p-0.5">
          {FORMATS.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className={cn(
                'h-7 rounded px-3 text-sm transition-colors',
                format === entry.id
                  ? 'bg-primary font-medium text-primary-foreground'
                  : 'text-muted-foreground hover:text-foreground'
              )}
              onClick={() => setFormat(entry.id)}
            >
              {t(entry.labelKey)}
            </button>
          ))}
        </div>
      </div>

      {preview !== null && (
        <pre className="max-h-48 overflow-auto rounded-md border border-border/70 bg-secondary/30 p-3 text-xs leading-relaxed text-foreground">
          {preview}
        </pre>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={() => void generateConfig()}
          disabled={isFetching}
        >
          {isFetching ? <Loader2 size={14} className="mr-1.5 animate-spin" /> : null}
          {t('memory.preview')}
        </Button>
        <Button type="button" size="sm" onClick={() => void handleCopy()} disabled={isFetching}>
          <Copy size={14} className="mr-1.5" />
          {t('memory.copy')}
        </Button>
      </div>
    </div>
  )
}
