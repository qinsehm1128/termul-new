import { Copy, Loader2 } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { toast } from '@/hooks/use-toast'
import { clipboardApi } from '@/lib/api'
import { logFrontendError } from '@/lib/log-api'
import {
  codexToml,
  type McpBridgeCommand,
  mcpGatewayApi,
  mcpServersJson
} from '@/lib/mcp-gateway-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { cn } from '@/lib/utils'

type Format = 'json' | 'toml'
type ExportError = 'loadFailed' | 'bridgeMissing' | 'copyFailed'

// Keep only the stdio bridge across the UI/clipboard boundary, never the HTTP token.
async function readBridge(): Promise<{ bridge: McpBridgeCommand } | { error: ExportError }> {
  try {
    const result = await mcpGatewayApi.connection('direct')
    if (!result.success || !result.data) return { error: 'loadFailed' }
    const { bridgeAvailable, bridge } = result.data
    if (
      !bridgeAvailable ||
      !bridge ||
      typeof bridge.command !== 'string' ||
      !bridge.command.trim() ||
      !Array.isArray(bridge.args) ||
      !bridge.args.every((arg) => typeof arg === 'string')
    ) {
      return { error: 'bridgeMissing' }
    }
    return { bridge: { command: bridge.command, args: [...bridge.args] } }
  } catch {
    return { error: 'loadFailed' }
  }
}

export function McpGatewayConfigSection(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const desktop = isTauriContext()
  const [format, setFormat] = useState<Format>('json')
  const [preview, setPreview] = useState<string | null>(null)
  const [error, setError] = useState<ExportError | null>(null)
  const [busy, setBusy] = useState(false)
  const mounted = useRef(false)
  const request = useRef(0)
  const pending = useRef(false)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      request.current += 1
    }
  }, [])

  function selectFormat(next: Format): void {
    if (next === format) return
    request.current += 1
    pending.current = false
    setBusy(false)
    setPreview(null)
    setError(null)
    setFormat(next)
  }

  async function generate(copy: boolean): Promise<void> {
    if (!desktop || pending.current) return
    const id = ++request.current
    const current = (): boolean => mounted.current && request.current === id
    pending.current = true
    setBusy(true)
    setPreview(null)
    setError(null)
    void logFrontendError({
      level: 'warn',
      source: 'McpGatewayConfigSection',
      message: 'Full MCP stdio export requested'
    })
    const fail = (reason: ExportError): void => {
      if (!current()) return
      setError(reason)
      void logFrontendError({
        source: 'McpGatewayConfigSection',
        message: `MCP stdio export failed: ${reason}`
      })
      toast({ title: t(`export.${reason}`), variant: 'destructive' })
    }
    try {
      const result = await readBridge()
      if (!current()) return
      if ('error' in result) {
        fail(result.error)
        return
      }
      const config = format === 'json' ? mcpServersJson(result.bridge) : codexToml(result.bridge)
      setPreview(config)
      if (!copy) return
      try {
        const copied = await clipboardApi.writeText(config)
        if (!current()) return
        if (!copied.success) {
          fail('copyFailed')
          return
        }
        toast({ title: t('export.copied') })
      } catch {
        fail('copyFailed')
      }
    } finally {
      if (current()) {
        pending.current = false
        setBusy(false)
      }
    }
  }

  return (
    <section className="min-w-0 space-y-3 rounded-lg border border-primary/40 bg-primary/5 p-4">
      <div>
        <h3 className="text-sm font-semibold text-foreground">{t('export.title')}</h3>
        <p className="text-xs font-medium text-foreground">{t('export.direct')}</p>
        <p className="mt-1 text-xs text-muted-foreground">{t('export.description')}</p>
      </div>
      <p className="text-xs text-muted-foreground">{t('export.authorization')}</p>
      <p className="text-xs text-muted-foreground">{t('export.externalOnly')}</p>
      {!desktop && (
        <p role="status" className="text-xs text-muted-foreground">
          {t('export.desktopOnly')}
        </p>
      )}
      <div>
        <span className="mb-1.5 block text-xs font-medium text-muted-foreground">
          {t('export.format')}
        </span>
        <div className="inline-flex h-8 items-center rounded-md border border-input/80 bg-secondary/35 p-0.5">
          {(['json', 'toml'] as const).map((entry) => (
            <button
              key={entry}
              type="button"
              aria-pressed={format === entry}
              className={cn(
                'h-7 rounded px-3 text-sm transition-colors',
                format === entry
                  ? 'bg-primary font-medium text-primary-foreground'
                  : 'text-muted-foreground hover:text-foreground'
              )}
              onClick={() => selectFormat(entry)}
            >
              {t(entry === 'json' ? 'export.formatJson' : 'export.formatToml')}
            </button>
          ))}
        </div>
      </div>
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {t(`export.${error}`)}
        </p>
      )}
      {preview !== null && (
        <pre className="max-h-64 overflow-auto rounded-md border border-border/70 bg-secondary/30 p-3 text-xs leading-relaxed text-foreground">
          {preview}
        </pre>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={() => void generate(false)}
          disabled={!desktop || busy}
        >
          {busy && <Loader2 size={14} className="mr-1.5 animate-spin" />}
          {t('export.preview')}
        </Button>
        <Button
          type="button"
          size="sm"
          onClick={() => void generate(true)}
          disabled={!desktop || busy}
        >
          <Copy size={14} className="mr-1.5" />
          {t('export.copy')}
        </Button>
      </div>
    </section>
  )
}
