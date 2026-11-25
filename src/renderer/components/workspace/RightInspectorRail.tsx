import { FolderTree, Server, TerminalSquare } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { SpringSlidingPill } from '@/components/ui/motion-primitives'
import { PanelFade } from '@/components/ui/panel-fade'
import { ResizableRail } from '@/components/workspace/ResizableRail'
import { cn } from '@/lib/utils'

export type RightInspectorSegment = 'files' | 'cli' | 'ssh'

export interface RightInspectorRailProps {
  hasFiles: boolean
  hasCli: boolean
  hasSsh: boolean
  storageKey: string
  resizeTitle: string
  resizeLabel: string
  filesContent: ReactNode
  cliContent: ReactNode
  sshContent: ReactNode
}

/**
 * Pure segment resolution helper so activation order and fallback are
 * deterministic and directly unit-testable.
 */
export function resolveInspectorSegment(
  current: RightInspectorSegment,
  available: readonly RightInspectorSegment[],
  newlyActivated: RightInspectorSegment | null
): RightInspectorSegment {
  if (newlyActivated && available.includes(newlyActivated)) {
    return newlyActivated
  }
  if (available.includes(current)) {
    return current
  }
  return available[0] ?? 'files'
}

/**
 * Unified right-hand inspector rail (Files / CLI / SSH).
 *
 * When a single tenant is open it fills 100% of the rail height with zero
 * redundant header chrome. When two or three tenants are active concurrently
 * (for example Files + SSH), a spring-animated segmented control switches
 * between them so each view keeps full vertical space instead of being crushed
 * into narrow stacked splits.
 */
export function RightInspectorRail({
  hasFiles,
  hasCli,
  hasSsh,
  storageKey,
  resizeTitle,
  resizeLabel,
  filesContent,
  cliContent,
  sshContent
}: RightInspectorRailProps): React.JSX.Element {
  const { t } = useTranslation('workspace')

  const available = useMemo<RightInspectorSegment[]>(() => {
    const list: RightInspectorSegment[] = []
    if (hasFiles) list.push('files')
    if (hasCli) list.push('cli')
    if (hasSsh) list.push('ssh')
    return list
  }, [hasFiles, hasCli, hasSsh])

  const [activeSegment, setActiveSegment] = useState<RightInspectorSegment>(() =>
    resolveInspectorSegment('files', available, null)
  )

  const prevFlagsRef = useRef({ hasFiles, hasCli, hasSsh })
  useEffect(() => {
    const prev = prevFlagsRef.current
    let newlyActivated: RightInspectorSegment | null = null
    if (hasSsh && !prev.hasSsh) newlyActivated = 'ssh'
    else if (hasCli && !prev.hasCli) newlyActivated = 'cli'
    else if (hasFiles && !prev.hasFiles) newlyActivated = 'files'

    prevFlagsRef.current = { hasFiles, hasCli, hasSsh }
    setActiveSegment((current) => resolveInspectorSegment(current, available, newlyActivated))
  }, [available, hasCli, hasFiles, hasSsh])

  const isOpen = available.length > 0
  const showSwitcher = available.length > 1
  const resolvedSegment = resolveInspectorSegment(activeSegment, available, null)

  const segmentLabel = (segment: RightInspectorSegment): string => {
    switch (segment) {
      case 'files':
        return t('inspector.files', { defaultValue: 'Files' })
      case 'cli':
        return t('inspector.cli', { defaultValue: 'CLI' })
      case 'ssh':
        return t('inspector.ssh', { defaultValue: 'SSH' })
    }
  }

  const segmentIcon = (segment: RightInspectorSegment): ReactNode => {
    switch (segment) {
      case 'files':
        return <FolderTree className="size-3 shrink-0" aria-hidden="true" />
      case 'cli':
        return <TerminalSquare className="size-3 shrink-0" aria-hidden="true" />
      case 'ssh':
        return <Server className="size-3 shrink-0" aria-hidden="true" />
    }
  }

  return (
    <PanelFade
      open={isOpen}
      data-testid="explorer-rail-fade"
      className="h-full min-h-0 border-l border-border/70"
    >
      <ResizableRail
        side="right"
        storageKey={storageKey}
        initial={268}
        min={228}
        max={560}
        className="bg-sidebar"
        resizeTitle={resizeTitle}
        resizeLabel={resizeLabel}
      >
        <div className="flex h-full min-h-0 flex-col">
          {showSwitcher && (
            <div className="flex h-9 shrink-0 items-center border-b border-border/70 px-2">
              <div
                role="tablist"
                aria-label={t('inspector.sectionsLabel', { defaultValue: 'Inspector panels' })}
                className="relative flex w-full items-center gap-0.5 rounded-md bg-muted/55 p-0.5 shadow-[inset_0_0_0_1px_hsl(var(--border)/0.55)]"
              >
                {available.map((segment) => {
                  const selected = resolvedSegment === segment
                  return (
                    <button
                      key={segment}
                      type="button"
                      role="tab"
                      aria-selected={selected}
                      onClick={() => setActiveSegment(segment)}
                      className={cn(
                        'relative flex h-6 flex-1 items-center justify-center gap-1.5 rounded-[5px] px-2 text-2xs font-medium transition-colors duration-150 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
                        selected ? 'text-foreground' : 'text-muted-foreground hover:text-foreground'
                      )}
                    >
                      {selected && (
                        <SpringSlidingPill
                          layoutId="right-inspector-segment"
                          className="bg-card shadow-[0_1px_2px_hsl(var(--background)/0.45),0_0_0_1px_hsl(var(--border)/0.8)]"
                        />
                      )}
                      <span className="relative z-[1] flex items-center gap-1.5">
                        {segmentIcon(segment)}
                        <span>{segmentLabel(segment)}</span>
                      </span>
                    </button>
                  )
                })}
              </div>
            </div>
          )}

          <div className="relative flex min-h-0 flex-1 flex-col overflow-hidden">
            <PanelFade
              open={hasCli}
              data-testid="cli-session-panel-fade"
              className={cn('h-full min-h-0 flex-1', resolvedSegment !== 'cli' && 'hidden')}
            >
              {cliContent}
            </PanelFade>

            <PanelFade
              open={hasFiles}
              data-testid="file-explorer-panel-fade"
              className={cn('h-full min-h-0 flex-1', resolvedSegment !== 'files' && 'hidden')}
            >
              {filesContent}
            </PanelFade>

            <PanelFade
              open={hasSsh}
              data-testid="ssh-explorer-panel-fade"
              className={cn('h-full min-h-0 flex-1', resolvedSegment !== 'ssh' && 'hidden')}
            >
              {sshContent}
            </PanelFade>
          </div>
        </div>
      </ResizableRail>
    </PanelFade>
  )
}
