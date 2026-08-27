import { ClipboardPaste, Keyboard, Monitor, Smartphone, ZoomIn, ZoomOut } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { useCompanionTerminalGeometry } from '@/hooks/use-companion-terminal-geometry'
import { useCompanionTerminalTextScale } from '@/hooks/use-companion-terminal-text-scale'
import { clipboardApi } from '@/lib/clipboard-api'
import { terminalApi } from '@/lib/terminal-api'
import { cn } from '@/lib/utils'

interface MobileTerminalControlsProps {
  terminalId: string
}

const KEYS = [
  ['Esc', '\u001b'],
  ['Tab', '\t'],
  ['Ctrl+C', '\u0003'],
  ['←', '\u001b[D'],
  ['↑', '\u001b[A'],
  ['↓', '\u001b[B'],
  ['→', '\u001b[C'],
  ['PgUp', '\u001b[5~'],
  ['PgDn', '\u001b[6~']
] as const

export function MobileTerminalControls({
  terminalId
}: MobileTerminalControlsProps): React.JSX.Element {
  const { t } = useTranslation('mobile')
  const [expanded, setExpanded] = useState(true)
  const textScale = useCompanionTerminalTextScale()
  const geometry = useCompanionTerminalGeometry()

  const write = async (data: string): Promise<void> => {
    const result = await terminalApi.write(terminalId, data)
    if (!result.success) {
      toast.error(t('terminalControls.writeFailed', { message: result.error }))
    }
  }

  const paste = async (): Promise<void> => {
    const result = await clipboardApi.readText()
    if (!result.success) {
      toast.error(t('terminalControls.clipboardReadFailed', { message: result.error }))
      return
    }
    if (result.data) {
      const writeResult = await terminalApi.write(terminalId, result.data)
      if (!writeResult.success) {
        toast.error(t('terminalControls.pasteFailed', { message: writeResult.error }))
      }
    }
  }

  return (
    <div className="shrink-0 border-t border-border/60 bg-card/95 px-2 pb-[max(0.5rem,env(safe-area-inset-bottom))] pt-1 backdrop-blur">
      <div className="flex items-center gap-1 overflow-x-auto scrollbar-hide">
        {geometry ? (
          <Button
            type="button"
            variant={geometry.preferredMode === 'phone' ? 'secondary' : 'ghost'}
            size="sm"
            className="h-10 shrink-0 gap-1 px-3"
            aria-label={
              geometry.preferredMode === 'phone'
                ? t('terminalControls.desktopLayout')
                : t('terminalControls.phoneLayout')
            }
            onClick={() =>
              geometry.setPreferredMode(geometry.preferredMode === 'phone' ? 'desktop' : 'phone')
            }
          >
            {geometry.preferredMode === 'phone' ? <Smartphone size={15} /> : <Monitor size={15} />}
            {geometry.preferredMode === 'phone'
              ? t('terminalControls.phoneLayout')
              : t('terminalControls.desktopLayout')}
          </Button>
        ) : null}
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-10 shrink-0 px-3"
          aria-label={expanded ? t('terminalControls.hideKeys') : t('terminalControls.showKeys')}
          aria-expanded={expanded}
          onClick={() => setExpanded((value) => !value)}
        >
          <Keyboard size={16} />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-10 shrink-0 px-3"
          aria-label={t('terminalControls.smallerText')}
          onClick={() => textScale.nudge(-1)}
        >
          <ZoomOut size={15} />
        </Button>
        <span
          role="status"
          className="inline-flex h-10 shrink-0 items-center px-2 text-xs tabular-nums text-muted-foreground"
          aria-live="polite"
          aria-label={t('terminalControls.textSize', { percent: textScale.percent })}
        >
          {textScale.percent}%
        </span>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-10 shrink-0 px-3"
          aria-label={t('terminalControls.largerText')}
          onClick={() => textScale.nudge(1)}
        >
          <ZoomIn size={15} />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-10 shrink-0 gap-1 px-3"
          onClick={() => void paste()}
        >
          <ClipboardPaste size={15} />
          {t('terminalControls.paste')}
        </Button>
        {KEYS.map(([label, data]) => (
          <Button
            key={label}
            type="button"
            variant="secondary"
            size="sm"
            className={cn('h-10 min-w-10 shrink-0 px-3 font-mono', !expanded && 'hidden')}
            onPointerDown={(event) => event.preventDefault()}
            onClick={() => void write(data)}
          >
            {label}
          </Button>
        ))}
      </div>
    </div>
  )
}
