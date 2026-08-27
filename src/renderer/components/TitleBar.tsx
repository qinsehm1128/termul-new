import { Copy, Minus, Square, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  CliSessionPanelToggleButton,
  FileExplorerToggleButton,
  SidebarToggleButton,
  titlebarNoDragStyle
} from '@/components/TitlebarPanelToggles'
import { windowApi } from '@/lib/api'
import { isMac } from '@/lib/platform'
import { useActiveProject } from '@/stores/project-store'

const windowControlClass =
  'inline-flex h-full w-10 cursor-pointer items-center justify-center text-muted-foreground transition-[color,background-color] duration-150 ease-[var(--ease-out)] hover:bg-foreground/[0.045] hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring/80'
/**
 * Slim window-control strip for Windows/Linux.
 *
 * The app runs with `decorations: false` on Windows/Linux, so the in-app
 * minimize/maximize/close controls are required. The strip sits at the top of
 * the content column (right of the ActivityRail) and doubles as a window drag
 * region. On macOS this renders nothing — native traffic lights handle window
 * controls, and WorkspaceLayout provides a unified top drag strip instead.
 *
 * Global actions (shortcuts, preferences) no longer live here; they moved to
 * the ActivityRail. The sidebar and file-explorer visibility toggles were
 * relocated back to this strip — left toggle at the far left, right toggle
 * just before the window controls — so they sit beside the OS window
 * controls instead of pinned to the bottom of the rail.
 */
export function TitleBar(): React.JSX.Element | null {
  const { t } = useTranslation('shell')
  const [isMaximized, setIsMaximized] = useState(false)
  const activeProject = useActiveProject()

  useEffect(() => {
    return windowApi.onMaximizeChange((maximized) => {
      setIsMaximized(maximized)
    })
  }, [])

  // macOS uses native traffic lights — no in-app window controls.
  if (isMac) return null

  return (
    <header
      className="relative flex h-8 shrink-0 select-none items-center border-b border-border/70 bg-sidebar shadow-[inset_0_1px_0_hsl(var(--foreground)/0.025)]"
      data-tauri-drag-region
    >
      {/* Left-sidebar toggle — top-left of the content column. */}
      <div className="relative z-10 flex h-full items-center" style={titlebarNoDragStyle}>
        <SidebarToggleButton />
      </div>

      {activeProject && (
        <span className="pointer-events-none absolute left-1/2 max-w-[50%] -translate-x-1/2 select-none truncate text-2xs font-medium tracking-[0.01em] text-muted-foreground/80">
          {activeProject.name}
        </span>
      )}

      <div className="flex-1 h-full" data-tauri-drag-region />

      {/* Right-sidebar toggle + window controls — top-right. */}
      <div className="relative z-10 flex h-full items-center" style={titlebarNoDragStyle}>
        <CliSessionPanelToggleButton />
        <FileExplorerToggleButton />

        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            void windowApi.minimize()
          }}
          className={windowControlClass}
          title={t('titleBar.minimize')}
          aria-label={t('titleBar.minimizeWindow')}
        >
          <Minus size={16} />
        </button>

        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            void windowApi.toggleMaximize().then((result) => {
              if (!result.success) {
                console.error(`Failed to toggle maximize: ${result.error ?? 'unknown error'}`)
              }
            })
          }}
          className={windowControlClass}
          title={isMaximized ? t('titleBar.restore') : t('titleBar.maximize')}
          aria-label={isMaximized ? t('titleBar.restoreWindow') : t('titleBar.maximizeWindow')}
        >
          {isMaximized ? <Copy size={14} /> : <Square size={14} />}
        </button>

        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            void windowApi.close()
          }}
          className="inline-flex h-full w-10 cursor-pointer items-center justify-center text-muted-foreground transition-[color,background-color] duration-150 ease-[var(--ease-out)] hover:bg-destructive/75 hover:text-destructive-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-destructive"
          title={t('titleBar.close')}
          aria-label={t('titleBar.closeWindow')}
        >
          <X size={16} />
        </button>
      </div>
    </header>
  )
}
