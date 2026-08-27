import { ArrowLeft, ArrowRight, Bug, Globe, Loader2, Pencil, RotateCcw } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import {
  browserTabGoBack,
  browserTabGoForward,
  browserTabOpenDevtools,
  browserTabReload
} from '@/lib/browser-api'
import { cn } from '@/lib/utils'
import { useBrowserSessionStore } from '@/stores/browser-session-store'

interface BrowserControlsProps {
  browserTabId: string
}

export function BrowserControls({ browserTabId }: BrowserControlsProps): React.JSX.Element {
  const { t } = useTranslation('browser')
  const tabUrl = useBrowserSessionStore((state) => state.tabs.get(browserTabId)?.url ?? '')
  const tabLoading = useBrowserSessionStore(
    (state) => state.tabs.get(browserTabId)?.loading ?? false
  )
  const tabAnnotationMode = useBrowserSessionStore(
    (state) => state.tabs.get(browserTabId)?.annotationMode ?? false
  )
  const [inputUrl, setInputUrl] = useState(tabUrl || '')

  // Sync inputUrl with store URL changes (e.g. from real-time sync)
  useEffect(() => {
    if (tabUrl) {
      setInputUrl(tabUrl)
    }
  }, [tabUrl])

  const handleNavigate = useCallback(() => {
    let url = inputUrl.trim()
    if (!url) return
    if (!/^https?:\/\//i.test(url) && !/^about:/i.test(url)) {
      url = `https://${url}`
    }
    useBrowserSessionStore.getState().updateUrl(browserTabId, url)
  }, [browserTabId, inputUrl])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        e.preventDefault()
        handleNavigate()
      }
    },
    [handleNavigate]
  )

  const handleToggleAnnotationMode = useCallback(() => {
    const currentMode = tabAnnotationMode
    useBrowserSessionStore.getState().setAnnotationMode(browserTabId, !currentMode)
  }, [browserTabId, tabAnnotationMode])

  if (!tabUrl) return <></>

  return (
    <div className="flex flex-col shrink-0">
      <div className="flex h-9 items-center gap-1.5 border-b border-border/70 bg-sidebar px-2 shadow-[inset_0_1px_0_hsl(var(--foreground)/0.025)]">
        <button
          onClick={() => browserTabGoBack(browserTabId).catch(console.error)}
          className="rounded-md p-1.5 text-muted-foreground transition-colors duration-150 hover:bg-secondary hover:text-foreground"
          title={t('controls.back')}
        >
          <ArrowLeft size={14} />
        </button>
        <button
          onClick={() => browserTabGoForward(browserTabId).catch(console.error)}
          className="rounded-md p-1.5 text-muted-foreground transition-colors duration-150 hover:bg-secondary hover:text-foreground"
          title={t('controls.forward')}
        >
          <ArrowRight size={14} />
        </button>
        <button
          onClick={() => browserTabReload(browserTabId).catch(console.error)}
          className="rounded-md p-1.5 text-muted-foreground transition-colors duration-150 hover:bg-secondary hover:text-foreground"
          title={t('controls.reload')}
        >
          <RotateCcw size={14} />
        </button>
        <div className="flex min-w-0 flex-1 items-center gap-2 rounded-md bg-secondary/35 px-2">
          {tabLoading ? (
            <Loader2 size={14} className="shrink-0 animate-spin text-primary" />
          ) : (
            <Globe size={14} className="shrink-0 text-muted-foreground" />
          )}
          <input
            type="text"
            value={inputUrl}
            onChange={(e) => setInputUrl(e.target.value)}
            onKeyDown={handleKeyDown}
            onBlur={handleNavigate}
            className="min-w-0 flex-1 bg-transparent text-sm text-foreground outline-none"
            placeholder={t('controls.enterUrl')}
          />
        </div>
        {!import.meta.env.PROD && (
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => browserTabOpenDevtools(browserTabId).catch(console.error)}
                className="shrink-0 rounded-md p-1.5 text-muted-foreground transition-colors duration-150 hover:bg-secondary hover:text-foreground"
                aria-label={t('controls.openDebugConsole')}
                title={t('controls.debugConsole')}
              >
                <Bug size={14} />
              </button>
            </TooltipTrigger>
            <TooltipContent side="bottom">{t('controls.debugConsole')}</TooltipContent>
          </Tooltip>
        )}
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              onClick={handleToggleAnnotationMode}
              aria-pressed={tabAnnotationMode}
              className={cn(
                'shrink-0 rounded-md p-1.5 transition-colors duration-150',
                tabAnnotationMode
                  ? 'bg-primary text-primary-foreground shadow-[inset_0_1px_0_hsl(var(--foreground)/0.08)] ring-1 ring-inset ring-primary/35 hover:bg-primary/90'
                  : 'text-muted-foreground hover:bg-secondary hover:text-foreground'
              )}
              aria-label={
                tabAnnotationMode
                  ? t('controls.disableAnnotationMode')
                  : t('controls.enableAnnotationMode')
              }
            >
              <Pencil size={14} />
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            {tabAnnotationMode
              ? t('controls.disableAnnotationMode')
              : t('controls.enableAnnotationMode')}
          </TooltipContent>
        </Tooltip>
      </div>
    </div>
  )
}
