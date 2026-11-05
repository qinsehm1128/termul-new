import { Maximize, Scan } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { ErrorBoundary } from '@/components/ErrorBoundary'
import { Button } from '@/components/ui/button'
import type { PreviewKind } from '@/lib/file-preview'
import { type ImagePreviewSource, loadImagePreview } from '@/lib/file-preview-api'
import { openerApi } from '@/lib/tauri-opener-api'
import { isTauriContext } from '@/lib/tauri-runtime'

interface FilePreviewProps {
  /** `svg` sources arrive as a data URL and render like any other image. */
  kind: PreviewKind
  src?: string
  filePath?: string
  version?: number
}

const CHECKERBOARD_STYLE = {
  backgroundImage: 'repeating-conic-gradient(hsl(var(--muted)) 0% 25%, transparent 0% 50%)',
  backgroundSize: '16px 16px'
}

export function PreviewFailure({
  onRetry,
  filePath,
  detail
}: {
  onRetry: () => void
  filePath?: string
  detail?: string
}): React.JSX.Element {
  const { t } = useTranslation('settings')
  const [revealError, setRevealError] = useState<string | null>(null)

  const revealOriginal = async (): Promise<void> => {
    if (!filePath) return
    setRevealError(null)
    try {
      const result = await openerApi.revealInFileManager(filePath)
      if (!result.success) setRevealError(result.error)
    } catch (error) {
      setRevealError(error instanceof Error ? error.message : String(error))
    }
  }

  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-3 p-6 text-sm text-muted-foreground">
      <p role="alert">{t('editor.previewFailed')}</p>
      {detail && <p>{detail}</p>}
      <div className="flex gap-2">
        <Button variant="outline" size="sm" onClick={onRetry}>
          {t('editor.previewRetry')}
        </Button>
        {filePath && isTauriContext() && (
          <Button variant="outline" size="sm" onClick={() => void revealOriginal()}>
            {t('editor.previewRevealOriginal')}
          </Button>
        )}
      </div>
      {filePath && (
        <p>{t(isTauriContext() ? 'editor.previewRevealHint' : 'editor.previewOpenOnDesktop')}</p>
      )}
      {revealError && <p role="alert">{revealError}</p>}
    </div>
  )
}

export function FilePreview(props: FilePreviewProps): React.JSX.Element {
  // A new source owns a fresh state tree: old load/error events cannot update the
  // new preview, and returning to a previously failed URL really retries it.
  return (
    <ErrorBoundary
      key={`${props.kind}:${props.filePath ?? ''}:${props.version ?? ''}:${props.src ?? ''}`}
      context="editorPane"
      fallback={(_error, retry) => <PreviewFailure filePath={props.filePath} onRetry={retry} />}
    >
      {props.kind === 'image' && props.filePath ? (
        <ImageFileSession filePath={props.filePath} />
      ) : (
        <PreviewSession {...props} />
      )}
    </ErrorBoundary>
  )
}

function ImageFileSession({ filePath }: { filePath: string }): React.JSX.Element {
  const [attempt, setAttempt] = useState(0)
  return (
    <ImageFileAttempt
      key={attempt}
      filePath={filePath}
      onRetry={() => setAttempt((value) => value + 1)}
    />
  )
}

function ImageFileAttempt({
  filePath,
  onRetry
}: {
  filePath: string
  onRetry: () => void
}): React.JSX.Element {
  const { t } = useTranslation('settings')
  const [source, setSource] = useState<ImagePreviewSource | null>(null)
  const [failure, setFailure] = useState<{ detail?: string } | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    let active = true
    let ownedSource: ImagePreviewSource | undefined
    void loadImagePreview(filePath, controller.signal)
      .then((result) => {
        if (!active) {
          if (result.success) result.data?.dispose()
          return
        }
        if (!result.success || !result.data) {
          setFailure({ detail: result.success ? undefined : result.error })
          return
        }
        ownedSource = result.data
        setSource(result.data)
      })
      .catch((error: unknown) => {
        if (active) setFailure({ detail: error instanceof Error ? error.message : String(error) })
      })
    return () => {
      active = false
      controller.abort()
      ownedSource?.dispose()
    }
  }, [filePath])

  if (failure)
    return <PreviewFailure filePath={filePath} onRetry={onRetry} detail={failure.detail} />
  if (!source)
    return (
      <div
        role="status"
        className="flex h-full items-center justify-center text-sm text-muted-foreground"
      >
        {t('editor.loading')}
      </div>
    )
  return (
    <ImagePreview
      src={source.url}
      onError={() => {
        source.dispose()
        setFailure({})
      }}
    />
  )
}

function PreviewSession(props: FilePreviewProps): React.JSX.Element {
  const [attempt, setAttempt] = useState(0)
  return (
    <PreviewAttempt
      key={attempt}
      {...props}
      attempt={attempt}
      onRetry={() => setAttempt((value) => value + 1)}
    />
  )
}

function PreviewAttempt({
  kind,
  src = '',
  filePath,
  attempt,
  onRetry
}: FilePreviewProps & {
  attempt: number
  onRetry: () => void
}): React.JSX.Element {
  const [failed, setFailed] = useState(false)
  // Revalidate disk/network sources on retry, preserving auth and version query
  // parameters. Inline SVG and blob URLs must remain byte-for-byte unchanged.
  const retrySrc =
    attempt > 0 && !/^(data:|blob:)/i.test(src)
      ? `${src}${src.includes('?') ? '&' : '?'}previewRetry=${attempt}`
      : src

  if (failed) return <PreviewFailure filePath={filePath} onRetry={onRetry} />
  const handleError = (): void => setFailed(true)

  if (kind === 'audio') {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        {/* biome-ignore lint/a11y/useMediaCaption: user files carry no caption track */}
        <audio
          key={attempt}
          className="w-full max-w-xl"
          controls
          src={retrySrc}
          onError={handleError}
        />
      </div>
    )
  }

  if (kind === 'video') {
    return (
      <div className="flex h-full w-full items-center justify-center bg-black/90 p-2">
        {/* biome-ignore lint/a11y/useMediaCaption: user files carry no caption track */}
        <video
          key={attempt}
          className="max-h-full max-w-full"
          controls
          src={retrySrc}
          onError={handleError}
        />
      </div>
    )
  }

  return <ImagePreview key={attempt} src={retrySrc} onError={handleError} />
}

function ImagePreview({ src, onError }: { src: string; onError: () => void }): React.JSX.Element {
  const { t } = useTranslation('settings')
  const [fitToWindow, setFitToWindow] = useState(true)
  const [size, setSize] = useState<{ width: number; height: number } | null>(null)

  return (
    <div className="flex h-full w-full flex-col">
      <div className="relative flex-1 overflow-auto" style={CHECKERBOARD_STYLE}>
        {!size && (
          <div
            role="status"
            className="absolute inset-x-0 top-2 text-center text-sm text-muted-foreground"
          >
            {t('editor.loading')}
          </div>
        )}
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
