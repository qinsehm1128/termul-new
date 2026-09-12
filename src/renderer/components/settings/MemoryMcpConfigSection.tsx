/**
 * "Memory MCP" export card (Settings → MCP Servers).
 *
 * The host's memory-index MCP server is this executable itself, run with a
 * per-project `--project/--state-root` argv that no user can be expected to
 * type — only the host knows its own paths (see memory_index/stdio_mcp.rs).
 * This card lets the user pick a project and copy the invocation in either
 * external-client shape: a Claude-style `mcpServers` JSON block or a Codex
 * `config.toml` `[mcp_servers.*]` table.
 *
 * Replaces the old project-sidebar context-menu copy, which only ever
 * produced a shell-quoted command line (a format no MCP client accepts).
 */

import { Copy, FolderOpen, Loader2 } from 'lucide-react'
import { useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { toast } from '@/hooks/use-toast'
import { clipboardApi, memoryIndexApi } from '@/lib/api'
import { buildMcpClientConfig, type McpClientConfigFormat } from '@/lib/mcp-client-config'
import { cn } from '@/lib/utils'
import { useProjects } from '@/stores/project-store'

const FORMATS = [
  { id: 'json', labelKey: 'memory.formatJson' },
  { id: 'toml', labelKey: 'memory.formatToml' }
] as const

export function MemoryMcpConfigSection(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const projects = useProjects()
  const projectsWithPath = useMemo(() => projects.filter((project) => project.path), [projects])
  const [selectedId, setSelectedId] = useState<string | null>(projectsWithPath[0]?.id ?? null)
  const [format, setFormat] = useState<McpClientConfigFormat>('json')
  const [isFetching, setIsFetching] = useState(false)
  const [preview, setPreview] = useState<string | null>(null)
  const fetchSeq = useRef(0)

  const selected =
    projectsWithPath.find((project) => project.id === selectedId) ?? projectsWithPath[0]

  async function generateConfig(): Promise<string | null> {
    if (!selected?.path) return null
    const seq = ++fetchSeq.current
    setIsFetching(true)
    try {
      const invocation = await memoryIndexApi.mcpInvocation({
        projectRoot: selected.path
      })
      if (seq !== fetchSeq.current) return null
      if (!invocation) {
        toast({ title: t('memory.unavailable'), variant: 'destructive' })
        return null
      }
      const config = buildMcpClientConfig(invocation, format, selected.name)
      setPreview(config)
      return config
    } catch (error) {
      if (seq === fetchSeq.current) {
        toast({
          title: t('memory.copyFailed'),
          description: error instanceof Error ? error.message : undefined,
          variant: 'destructive'
        })
      }
      return null
    } finally {
      if (seq === fetchSeq.current) setIsFetching(false)
    }
  }

  async function handleCopy(): Promise<void> {
    const config = (await generateConfig()) ?? preview
    if (!config) return
    await clipboardApi.writeText(config)
    toast({ title: t('memory.copied'), description: selected?.name })
  }

  return (
    <div className="space-y-4 rounded-lg border border-border bg-secondary/20 p-4">
      <div>
        <p className="text-sm font-medium text-foreground">{t('memory.title')}</p>
        <p className="text-xs text-muted-foreground">{t('memory.description')}</p>
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        <div>
          <label
            htmlFor="memory-mcp-project"
            className="mb-1.5 block text-xs font-medium text-muted-foreground"
          >
            {t('memory.project')}
          </label>
          <div className="relative">
            <FolderOpen
              size={14}
              className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground"
            />
            <select
              id="memory-mcp-project"
              value={selected?.id ?? ''}
              disabled={projectsWithPath.length === 0}
              onChange={(event) => setSelectedId(event.target.value)}
              className="h-8 w-full appearance-none rounded-md border border-input/80 bg-secondary/35 pl-8 pr-6 text-sm text-foreground outline-none focus-visible:border-ring/70"
            >
              {projectsWithPath.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
            </select>
          </div>
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
      </div>

      {projectsWithPath.length === 0 && (
        <p className="text-xs text-muted-foreground">{t('memory.noProjects')}</p>
      )}

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
          disabled={isFetching || projectsWithPath.length === 0}
        >
          {isFetching ? <Loader2 size={14} className="mr-1.5 animate-spin" /> : null}
          {t('memory.preview')}
        </Button>
        <Button
          type="button"
          size="sm"
          onClick={() => void handleCopy()}
          disabled={isFetching || projectsWithPath.length === 0}
        >
          <Copy size={14} className="mr-1.5" />
          {t('memory.copy')}
        </Button>
      </div>
    </div>
  )
}
