import {
  Code2,
  ExternalLink,
  Eye,
  FileText,
  Maximize2,
  Minimize2,
  Minus,
  Save,
  X
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { getPreviewKind, isDiskPreview, previewSourceUrl, svgDataUrl } from '@/lib/file-preview'
import { isTauriContext } from '@/lib/tauri-runtime'
import { cn } from '@/lib/utils'
import { type EditorFileState, useEditorStore } from '@/stores/editor-store'
import {
  clampBallPosition,
  FLOATING_BALL_MARGIN,
  FLOATING_BALL_SIZE,
  type FloatingDocPoint,
  useFloatingDocStore
} from '@/stores/floating-doc-store'
import { CodeEditor } from './CodeEditor'
import { FilePreview } from './FilePreview'
import { HtmlPreview } from './HtmlPreview'
import { MarkdownEditor } from './MarkdownEditor'

/** How a previewable document is shown: rendered output or its source text. */
type RenderMode = 'rendered' | 'source'

type PendingClose = { kind: 'one'; path: string } | { kind: 'all' }

function fileNameOf(path: string): string {
  return path.split(/[\\/]/).pop() || path
}

function isMarkdownPath(path: string): boolean {
  return /\.(md|markdown|mdx)$/i.test(path)
}

function isHtmlPath(path: string): boolean {
  // `getPreviewKind` reports null for HTML outside Tauri (no asset protocol),
  // so the preview window decides from the extension instead.
  return /\.(html?|xhtml)$/i.test(path) || getPreviewKind(path) === 'html'
}

function isSvgPath(path: string): boolean {
  return getPreviewKind(path) === 'svg'
}

function hasRenderableView(path: string): boolean {
  return isMarkdownPath(path) || isHtmlPath(path) || isSvgPath(path)
}

function defaultRenderMode(path: string): RenderMode {
  return hasRenderableView(path) ? 'rendered' : 'source'
}

function defaultBallPoint(): FloatingDocPoint {
  if (typeof window === 'undefined') return { x: FLOATING_BALL_MARGIN, y: FLOATING_BALL_MARGIN }
  return clampBallPosition({
    x: window.innerWidth - FLOATING_BALL_SIZE - FLOATING_BALL_MARGIN * 2,
    y: window.innerHeight - FLOATING_BALL_SIZE - FLOATING_BALL_MARGIN * 4
  })
}

/** Snap to the nearest horizontal edge so a dropped ball never crowds content. */
function snapToEdge(point: FloatingDocPoint): FloatingDocPoint {
  if (typeof window === 'undefined') return point
  const clamped = clampBallPosition(point)
  const center = window.innerWidth / 2
  return snapHorizontally(clamped, center, window.innerWidth)
}

function snapHorizontally(
  point: FloatingDocPoint,
  center: number,
  viewportWidth: number
): FloatingDocPoint {
  const snap = point.x + FLOATING_BALL_SIZE / 2 < center
  return clampBallPosition({
    x: snap ? FLOATING_BALL_MARGIN : viewportWidth - FLOATING_BALL_SIZE - FLOATING_BALL_MARGIN,
    y: point.y
  })
}

function BallBadge({ count, dirty }: { count: number; dirty: boolean }): React.JSX.Element {
  return (
    <>
      {count > 0 ? (
        <span className="absolute -right-1 -top-1 flex h-5 min-w-5 items-center justify-center rounded-full bg-primary px-1 text-[10px] font-semibold leading-none text-primary-foreground shadow">
          {count > 9 ? '9+' : count}
        </span>
      ) : null}
      {dirty ? (
        <span className="absolute -left-0.5 -top-0.5 h-2.5 w-2.5 rounded-full bg-amber-500 shadow" />
      ) : null}
    </>
  )
}

export function FloatingDocPreview(): React.JSX.Element | null {
  const { t } = useTranslation('settings')
  const docPaths = useFloatingDocStore((state) => state.docPaths)
  const activePath = useFloatingDocStore((state) => state.activePath)
  const mode = useFloatingDocStore((state) => state.mode)
  const ballPosition = useFloatingDocStore((state) => state.ballPosition)
  const rect = useFloatingDocStore((state) => state.rect)
  const openFiles = useEditorStore((state) => state.openFiles)

  const [renderModes, setRenderModes] = useState<Record<string, RenderMode>>({})
  const [pendingClose, setPendingClose] = useState<PendingClose | null>(null)
  const [dragPoint, setDragPoint] = useState<FloatingDocPoint | null>(null)
  const rootRef = useRef<HTMLDivElement>(null)
  const suppressClickRef = useRef(false)
  const dragRef = useRef<{
    pointerId: number
    startX: number
    startY: number
    originX: number
    originY: number
    moved: boolean
  } | null>(null)

  const activeFile = activePath ? openFiles.get(activePath) : undefined
  const activeRenderMode: RenderMode = activePath
    ? (renderModes[activePath] ?? defaultRenderMode(activePath))
    : 'source'
  const dirtyCount = useMemo(
    () => docPaths.filter((path) => openFiles.get(path)?.isDirty).length,
    [docPaths, openFiles]
  )

  const closeDocument = useFloatingDocStore((state) => state.closeDocument)
  const pruneMissingDocuments = useFloatingDocStore((state) => state.pruneMissingDocuments)
  const closeAllDocuments = useFloatingDocStore((state) => state.closeAllDocuments)
  const setActiveDoc = useFloatingDocStore((state) => state.setActiveDoc)
  const setMode = useFloatingDocStore((state) => state.setMode)
  const toggleFullscreen = useFloatingDocStore((state) => state.toggleFullscreen)
  const setBallPosition = useFloatingDocStore((state) => state.setBallPosition)
  const moveToWorkspaceTab = useFloatingDocStore((state) => state.moveToWorkspaceTab)
  const saveFile = useEditorStore((state) => state.saveFile)

  const requestClose = useCallback(
    (path: string) => {
      if (openFiles.get(path)?.isDirty) {
        setPendingClose({ kind: 'one', path })
        return
      }
      closeDocument(path)
    },
    [closeDocument, openFiles]
  )

  const requestCloseAll = useCallback(() => {
    if (dirtyCount > 0) {
      // The confirm bar lives in the window, never behind the ball.
      setMode('floating')
      setPendingClose({ kind: 'all' })
      return
    }
    closeAllDocuments()
  }, [closeAllDocuments, dirtyCount, setMode])

  const finishPendingClose = useCallback(
    async (save: boolean) => {
      const pending = pendingClose
      setPendingClose(null)
      if (!pending) return
      const paths = pending.kind === 'one' ? [pending.path] : [...docPaths]
      for (const path of paths) {
        const isDirty = Boolean(useEditorStore.getState().openFiles.get(path)?.isDirty)
        if (save && isDirty) {
          const ok = await saveFile(path)
          if (!ok) {
            toast.error(t('floatingDoc.saveFailed', { path }))
            continue
          }
        }
        closeDocument(path, { discard: !save && isDirty })
      }
    },
    [closeDocument, docPaths, pendingClose, saveFile, t]
  )

  useEffect(() => {
    if (docPaths.some((path) => !openFiles.has(path))) {
      pruneMissingDocuments()
    }
  }, [docPaths, openFiles, pruneMissingDocuments])

  useEffect(() => {
    if (typeof window === 'undefined') return
    const handleResize = (): void => {
      if (ballPosition) setBallPosition(ballPosition)
    }
    window.addEventListener('resize', handleResize)
    return () => window.removeEventListener('resize', handleResize)
  }, [ballPosition, setBallPosition])

  // Cmd/Ctrl+S saves the previewed document while focus is inside the window.
  useEffect(() => {
    if (mode === 'collapsed' || !activePath) return
    const handleKeyDown = (event: KeyboardEvent): void => {
      const root = rootRef.current
      if (!root?.contains(document.activeElement)) return
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
        event.preventDefault()
        void saveFile(activePath)
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [activePath, mode, saveFile])

  // Escape dismisses the confirm bar first, then leaves full screen.
  useEffect(() => {
    if (mode === 'collapsed' && !pendingClose) return
    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return
      if (pendingClose) {
        setPendingClose(null)
        return
      }
      if (mode === 'fullscreen') setMode('floating')
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [mode, pendingClose, setMode])

  const handlePointerDown = useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      const origin = ballPosition ?? defaultBallPoint()
      dragRef.current = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        originX: origin.x,
        originY: origin.y,
        moved: false
      }
      event.currentTarget.setPointerCapture?.(event.pointerId)
    },
    [ballPosition]
  )

  const handlePointerMove = useCallback((event: React.PointerEvent<HTMLButtonElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    const dx = event.clientX - drag.startX
    const dy = event.clientY - drag.startY
    if (Math.abs(dx) > 3 || Math.abs(dy) > 3) {
      drag.moved = true
      suppressClickRef.current = true
    }
    setDragPoint(clampBallPosition({ x: drag.originX + dx, y: drag.originY + dy }))
  }, [])

  const handlePointerUp = useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      const drag = dragRef.current
      dragRef.current = null
      event.currentTarget.releasePointerCapture?.(event.pointerId)
      if (!drag) return
      if (drag.moved) {
        const dropped = dragPoint ?? { x: drag.originX, y: drag.originY }
        setBallPosition(snapToEdge(dropped))
        setDragPoint(null)
        return
      }
      setDragPoint(null)
      setMode('floating')
    },
    [dragPoint, setBallPosition, setMode]
  )

  const handlePointerCancel = useCallback((event: React.PointerEvent<HTMLButtonElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    dragRef.current = null
    suppressClickRef.current = false
    setDragPoint(null)
    event.currentTarget.releasePointerCapture?.(event.pointerId)
  }, [])

  if (docPaths.length === 0) return null

  const activeName = activePath ? fileNameOf(activePath) : ''

  if (mode === 'collapsed') {
    const point = dragPoint ?? ballPosition ?? defaultBallPoint()
    return (
      <div
        ref={rootRef}
        data-testid="floating-doc-root"
        className="fixed z-50"
        style={{ left: point.x, top: point.y }}
      >
        <div className="group relative">
          <button
            type="button"
            data-testid="floating-doc-ball"
            aria-label={t('floatingDoc.ballAria', {
              count: docPaths.length,
              name: activeName
            })}
            title={activeName}
            onPointerDown={handlePointerDown}
            onPointerMove={handlePointerMove}
            onPointerUp={handlePointerUp}
            onPointerCancel={handlePointerCancel}
            onClick={() => {
              // Keyboard and assistive activation arrive as a click; a drag that
              // just ended must not also expand the window.
              if (suppressClickRef.current) {
                suppressClickRef.current = false
                return
              }
              if (!dragRef.current) setMode('floating')
            }}
            onDoubleClick={() => setMode('floating')}
            className="relative flex h-11 w-11 cursor-grab touch-none items-center justify-center rounded-full border border-border bg-sidebar text-sidebar-foreground shadow-lg transition-transform hover:scale-105 active:cursor-grabbing"
            style={{ width: FLOATING_BALL_SIZE, height: FLOATING_BALL_SIZE }}
          >
            <FileText size={18} />
            <BallBadge count={docPaths.length} dirty={dirtyCount > 0} />
          </button>
          <button
            type="button"
            aria-label={t('floatingDoc.closeAll')}
            title={t('floatingDoc.closeAll')}
            onClick={requestCloseAll}
            className="absolute -right-1.5 -top-1.5 flex h-5 w-5 items-center justify-center rounded-full border border-border bg-background text-muted-foreground opacity-0 shadow transition-opacity hover:text-foreground group-hover:opacity-100"
          >
            <X size={11} />
          </button>
        </div>
      </div>
    )
  }

  const isFullscreen = mode === 'fullscreen'
  const containerClass = isFullscreen
    ? 'fixed inset-0 z-50 flex flex-col bg-background'
    : 'fixed z-50 flex flex-col overflow-hidden rounded-xl border border-border bg-background shadow-2xl'
  const containerStyle = isFullscreen
    ? undefined
    : {
        left: rect.x,
        top: rect.y,
        width: `min(${rect.width}px, calc(100vw - 1rem))`,
        height: `min(${rect.height}px, calc(100vh - 1rem))`
      }

  return (
    <div
      ref={rootRef}
      role="dialog"
      aria-label={t('floatingDoc.title', 'Document Preview')}
      data-testid="floating-doc-root"
      tabIndex={-1}
      className={containerClass}
      style={containerStyle}
      onMouseDown={() => {
        // Move keyboard focus into the window so the save shortcut applies.
        if (!rootRef.current?.contains(document.activeElement)) {
          rootRef.current?.focus()
        }
      }}
    >
      <div className="flex shrink-0 items-center gap-2 border-b border-border/70 bg-sidebar px-2 py-1.5">
        <div
          data-testid="floating-doc-tabs"
          className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto"
          role="tablist"
          aria-label={t('floatingDoc.tabsAria')}
        >
          {docPaths.map((path) => {
            const file = openFiles.get(path)
            const selected = path === activePath
            return (
              <div
                key={path}
                role="tab"
                aria-selected={selected}
                tabIndex={0}
                onClick={() => setActiveDoc(path)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault()
                    setActiveDoc(path)
                  }
                }}
                className={cn(
                  'group flex shrink-0 cursor-pointer items-center gap-1 rounded-md px-2 py-1 text-xs transition-colors',
                  selected
                    ? 'bg-sidebar-accent text-foreground'
                    : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground'
                )}
                title={path}
              >
                <span className="max-w-[10rem] truncate">{fileNameOf(path)}</span>
                {file?.isDirty ? (
                  <span
                    className="h-1.5 w-1.5 rounded-full bg-amber-500"
                    role="img"
                    aria-label={t('floatingDoc.unsavedDot')}
                    title={t('floatingDoc.unsavedDot')}
                  />
                ) : null}
                <button
                  type="button"
                  aria-label={t('floatingDoc.close')}
                  title={t('floatingDoc.close')}
                  onClick={(event) => {
                    event.stopPropagation()
                    requestClose(path)
                  }}
                  className="rounded p-0.5 text-muted-foreground opacity-60 hover:bg-background/70 hover:text-foreground group-hover:opacity-100"
                >
                  <X size={12} />
                </button>
              </div>
            )
          })}
        </div>

        <div className="flex shrink-0 items-center gap-0.5">
          {activePath && hasRenderableView(activePath) ? (
            <Button
              variant="ghost"
              size="sm"
              className={cn(
                'h-7 px-2 text-xs text-muted-foreground hover:text-foreground',
                activeRenderMode === 'rendered' && 'bg-sidebar-accent text-foreground'
              )}
              aria-label={
                activeRenderMode === 'rendered'
                  ? t('floatingDoc.source')
                  : t('floatingDoc.rendered')
              }
              title={
                activeRenderMode === 'rendered'
                  ? t('floatingDoc.source')
                  : t('floatingDoc.rendered')
              }
              onClick={() =>
                activePath &&
                setRenderModes((current) => ({
                  ...current,
                  [activePath]: activeRenderMode === 'rendered' ? 'source' : 'rendered'
                }))
              }
            >
              {activeRenderMode === 'rendered' ? <Code2 size={14} /> : <Eye size={14} />}
            </Button>
          ) : null}

          <Button
            variant="ghost"
            size="sm"
            className={cn(
              'h-7 px-2 text-xs hover:text-foreground',
              activeFile?.isDirty ? 'text-amber-600' : 'text-muted-foreground'
            )}
            disabled={!activePath || !activeFile?.isDirty}
            aria-label={t('floatingDoc.save')}
            title={t('floatingDoc.save')}
            onClick={() => activePath && void saveFile(activePath)}
          >
            <Save size={14} />
          </Button>

          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground"
            aria-label={t('floatingDoc.openInWorkspace')}
            title={t('floatingDoc.openInWorkspace')}
            onClick={() => activePath && moveToWorkspaceTab(activePath)}
          >
            <ExternalLink size={14} />
          </Button>

          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground"
            aria-label={
              isFullscreen ? t('floatingDoc.exitFullscreen') : t('floatingDoc.fullscreen')
            }
            title={isFullscreen ? t('floatingDoc.exitFullscreen') : t('floatingDoc.fullscreen')}
            onClick={toggleFullscreen}
          >
            {isFullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
          </Button>

          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground"
            aria-label={t('floatingDoc.collapse')}
            title={t('floatingDoc.collapse')}
            onClick={() => setMode('collapsed')}
          >
            <Minus size={14} />
          </Button>

          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs text-muted-foreground hover:text-destructive"
            aria-label={t('floatingDoc.closeAll')}
            title={t('floatingDoc.closeAll')}
            onClick={requestCloseAll}
          >
            <X size={14} />
          </Button>
        </div>
      </div>

      <div data-testid="floating-doc-content" className="relative min-h-0 flex-1 overflow-hidden">
        {activePath && activeFile ? (
          <DocumentBody
            key={`${activePath}:${activeRenderMode}`}
            filePath={activePath}
            file={activeFile}
            renderMode={activeRenderMode}
          />
        ) : (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            {t('editor.loading')}
          </div>
        )}
      </div>

      {pendingClose ? (
        <div
          role="alertdialog"
          aria-label={t('floatingDoc.unsavedWarning')}
          data-testid="floating-doc-confirm"
          className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-border/70 bg-secondary/40 px-3 py-2 text-xs"
        >
          <span className="mr-auto text-muted-foreground">{t('floatingDoc.unsavedWarning')}</span>
          <Button
            size="sm"
            className="h-7 px-2.5 text-xs"
            onClick={() => void finishPendingClose(true)}
          >
            {t('floatingDoc.saveAndClose')}
          </Button>
          <Button
            size="sm"
            variant="outline"
            className="h-7 px-2.5 text-xs"
            onClick={() => void finishPendingClose(false)}
          >
            {t('floatingDoc.discardAndClose')}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 px-2.5 text-xs"
            onClick={() => setPendingClose(null)}
          >
            {t('floatingDoc.cancel')}
          </Button>
        </div>
      ) : null}
    </div>
  )
}

function DocumentBody({
  filePath,
  file,
  renderMode
}: {
  filePath: string
  file: EditorFileState
  renderMode: RenderMode
}): React.JSX.Element {
  const updateContent = useEditorStore((state) => state.updateContent)
  const updateCursorPosition = useEditorStore((state) => state.updateCursorPosition)
  const updateScrollTop = useEditorStore((state) => state.updateScrollTop)
  const previewKind = getPreviewKind(filePath)

  if (isDiskPreview(previewKind) && previewKind) {
    return previewKind === 'image' ? (
      <FilePreview kind={previewKind} filePath={filePath} version={file.lastModified} />
    ) : (
      <FilePreview
        kind={previewKind}
        src={previewSourceUrl(filePath, file.lastModified)}
        filePath={filePath}
      />
    )
  }

  const name = fileNameOf(filePath)
  const onChange = (content: string): void => updateContent(filePath, content)

  if (renderMode === 'rendered') {
    if (isMarkdownPath(filePath)) {
      return (
        <MarkdownEditor filePath={filePath} content={file.content} isVisible onChange={onChange} />
      )
    }
    if (isHtmlPath(filePath)) {
      const inline = file.isDirty || !isTauriContext()
      return (
        <HtmlPreview
          {...(inline
            ? { srcDoc: file.content }
            : { src: previewSourceUrl(filePath, file.lastModified) })}
          name={name}
          unsaved={file.isDirty}
        />
      )
    }
    if (isSvgPath(filePath)) {
      return <FilePreview kind="svg" src={svgDataUrl(file.content)} filePath={filePath} />
    }
  }

  return (
    <CodeEditor
      filePath={filePath}
      content={file.content}
      language={file.language}
      isVisible
      initialCursorPosition={file.cursorPosition}
      initialScrollTop={file.scrollTop}
      onChange={onChange}
      onCursorChange={(line, col) => updateCursorPosition(filePath, line, col)}
      onScrollChange={(scrollTop) => updateScrollTop(filePath, scrollTop)}
    />
  )
}
