import type { McpHostAccessPolicy, McpHostAccessView } from '@shared/types/mcp-host.types'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import { HostAccessSection, hostCheckboxClass, hostInputClass } from './McpHostAccessSection'

type WorkspaceGrant = McpHostAccessPolicy['workspace']

export function McpWorkspaceAccess({
  value,
  projects,
  disabled,
  onChange
}: {
  value: WorkspaceGrant
  projects: McpHostAccessView['projects']
  disabled: boolean
  onChange: (value: WorkspaceGrant) => void
}): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const [open, setOpen] = useState(true)
  const [query, setQuery] = useState('')
  const [selectedOnly, setSelectedOnly] = useState(false)
  const all = value.projectIds === null
  const selected = new Set(value.projectIds ?? [])
  const term = query.trim().toLocaleLowerCase()
  const filtered = projects.filter(
    (project) =>
      (!selectedOnly || selected.has(project.id)) &&
      (!term || `${project.name} ${project.id}`.toLocaleLowerCase().includes(term))
  )
  const count = all ? projects.length : projects.filter((p) => selected.has(p.id)).length
  const names = new Map<string, number>()
  for (const project of projects) names.set(project.name, (names.get(project.name) ?? 0) + 1)
  function selectResults(): void {
    onChange({ ...value, projectIds: [...new Set([...selected, ...filtered.map((p) => p.id)])] })
  }
  return (
    <HostAccessSection
      title={t('hostAccess.workspace')}
      open={open}
      onOpenChange={setOpen}
      summary={
        all
          ? t('hostAccess.allProjectsShort')
          : t('hostAccess.selectedCount', { selected: count, total: projects.length })
      }
    >
      <fieldset disabled={disabled} className="min-w-0 space-y-3">
        <label className="flex min-h-8 items-center gap-2 text-sm">
          <input
            className={hostCheckboxClass}
            type="checkbox"
            checked={value.enabled}
            onChange={(e) => onChange({ ...value, enabled: e.target.checked })}
          />
          {t('hostAccess.workspaceEnabled')}
        </label>
        <label className="flex min-h-8 items-center gap-2 text-xs">
          <input
            className={hostCheckboxClass}
            type="checkbox"
            checked={all}
            onChange={(e) => onChange({ ...value, projectIds: e.target.checked ? null : [] })}
          />
          {t('hostAccess.allProjects')}
        </label>
        {all ? (
          <p className="rounded-md bg-secondary/50 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
            {t('hostAccess.futureProjectsHint')}
          </p>
        ) : (
          <div className="space-y-2">
            <input
              type="search"
              name="mcp-project-filter"
              autoComplete="off"
              spellCheck={false}
              aria-label={t('hostAccess.searchProjects')}
              placeholder={t('hostAccess.searchProjects')}
              className={hostInputClass}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={disabled || filtered.length === 0}
                onClick={selectResults}
              >
                {t('hostAccess.selectResults')}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="ghost"
                disabled={disabled || selected.size === 0}
                onClick={() => onChange({ ...value, projectIds: [] })}
              >
                {t('hostAccess.clearSelection')}
              </Button>
              <label className="ml-auto flex min-h-8 items-center gap-2 text-xs text-muted-foreground">
                <input
                  className={hostCheckboxClass}
                  type="checkbox"
                  checked={selectedOnly}
                  onChange={(e) => setSelectedOnly(e.target.checked)}
                />
                {t('hostAccess.selectedOnly')}
              </label>
            </div>
            <fieldset
              className="grid max-h-52 min-w-0 grid-cols-1 gap-x-2 overflow-y-auto overscroll-contain rounded-md border border-border p-1 sm:grid-cols-2"
              aria-label={t('hostAccess.projectChoices')}
            >
              {filtered.map((project) => (
                <label
                  key={project.id}
                  className="flex min-h-9 min-w-0 cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-xs hover:bg-secondary/60"
                  style={{ contentVisibility: 'auto', containIntrinsicSize: 'auto 36px' }}
                >
                  <input
                    className={hostCheckboxClass}
                    type="checkbox"
                    checked={selected.has(project.id)}
                    onChange={(e) => {
                      const next = new Set(selected)
                      if (e.target.checked) next.add(project.id)
                      else next.delete(project.id)
                      onChange({ ...value, projectIds: [...next] })
                    }}
                  />
                  <span className="min-w-0 break-words">
                    {project.name}
                    {(names.get(project.name) ?? 0) > 1 && (
                      <span className="ml-2 font-mono text-[10px] text-muted-foreground">
                        {project.id.slice(-8)}
                      </span>
                    )}
                  </span>
                </label>
              ))}
              {filtered.length === 0 && (
                <p className="col-span-full px-3 py-5 text-center text-xs text-muted-foreground">
                  {t(projects.length ? 'hostAccess.noMatches' : 'hostAccess.noProjects')}
                </p>
              )}
            </fieldset>
            <p className="text-xs leading-relaxed text-muted-foreground">
              {t('hostAccess.selectResultsHint')}
            </p>
          </div>
        )}
        <label className="flex min-h-8 items-center gap-2 border-t border-border pt-3 text-xs">
          <input
            className={hostCheckboxClass}
            type="checkbox"
            checked={value.includePaths}
            onChange={(e) => onChange({ ...value, includePaths: e.target.checked })}
          />
          {t('hostAccess.includePaths')}
        </label>
      </fieldset>
    </HostAccessSection>
  )
}
