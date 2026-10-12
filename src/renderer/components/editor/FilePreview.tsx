import { Maximize, Scan } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '@/components/ui/button'
import type { PreviewKind } from '@/lib/file-preview'

interface FilePreviewProps {
  /** `svg` sources arrive as a data URL and render like any other image. */
  kind: PreviewKind
  src: string
}

const CHECKERBOARD_STYLE = {
  backgroundImage: 'repeating-conic-gradient(hsl(var(--muted)) 0% 25%, transparent 0% 50%)',
  backgroundSize: '16px 16px'
}

export function FilePreview({ kind, src }: FilePreviewProps): React.JSX.Element {
  const { t } = useTranslation('settings')
  const [failedSrc, setFailedSrc] = useState<string | null>(null)

  if (failedSrc === src) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6 text-sm text-muted-foreground">
        {t('editor.previewFailed')}
      </div>
    )
  }

  const handleError = (): void => setFailedSrc(src)

  if (kind === 'audio') {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        {/* biome-ignore lint/a11y/useMediaCaption: user files carry no caption track */}
        <audio className="w-full max-w-xl" controls src={src} onError={handleError} />
      </div>
    )
  }

  if (kind === 'video') {
    return (
      <div className="flex h-full w-full items-center justify-center bg-black/90 p-2">
        {/* biome-ignore lint/a11y/useMediaCaption: user files carry no caption track */}
        <video className="max-h-full max-w-full" controls src={src} onError={handleError} />
      </div>
    )
  }

  return <ImagePreview src={src} onError={handleError} />
}

function ImagePreview({ src, onError }: { src: string; onError: () => void }): React.JSX.Element {
  const { t } = useTranslation('settings')
  const [fitToWindow, setFitToWindow] = useState(true)
  const [size, setSize] = useState<{ width: number; height: number } | null>(null)

  return (
    <div className="flex h-full w-full flex-col">
      <div className="relative flex-1 overflow-auto" style={CHECKERBOARD_STYLE}>
        <div
          className={
            fitToWindow
              ? 'absolute inset-0 flex items-center justify-center p-4'
              : 'flex min-h-full w-max min-w-full items-center justify-center p-4'
          }
        >
          <img
            src={src}
            alt=""
            draggable={false}
            className={fitToWindow ? 'max-h-full max-w-full object-contain' : 'max-w-none'}
            onLoad={(event) =>
              setSize({
                width: event.currentTarget.naturalWidth,
                height: event.currentTarget.naturalHeight
              })
            }
            onError={onError}
          />
        </div>
      </div>
      <div className="flex h-7 shrink-0 items-center justify-between border-t border-border/70 bg-sidebar px-2.5 text-xs text-muted-foreground">
        <span>{size ? `${size.width} × ${size.height}` : ''}</span>
        <Button
          variant="ghost"
          size="sm"
          className="h-5 gap-1 px-2 text-xs text-muted-foreground hover:text-foreground"
          onClick={() => setFitToWindow((value) => !value)}
        >
          {fitToWindow ? <Scan size={12} /> : <Maximize size={12} />}
          <span>{fitToWindow ? t('editor.actualSize') : t('editor.fitToWindow')}</span>
        </Button>
      </div>
    </div>
  )
}
