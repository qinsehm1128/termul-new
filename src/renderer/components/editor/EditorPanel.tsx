import { useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import { useTocSettings } from '@/hooks/use-toc-settings'
import { getPreviewKind, isDiskPreview, previewSourceUrl, svgDataUrl } from '@/lib/file-preview'
import type { EditorFileState } from '@/stores/editor-store'
import { useEditorStore } from '@/stores/editor-store'
import { CodeEditor } from './CodeEditor'
import { EditorToolbar } from './EditorToolbar'
import { FilePreview } from './FilePreview'
import { HtmlPreview } from './HtmlPreview'
import { MarkdownEditor } from './MarkdownEditor'

interface EditorPanelProps {
  filePath: string
  isVisible: boolean
}

export function EditorPanel({ filePath, isVisible }: EditorPanelProps): React.JSX.Element {
  const { t } = useTranslation('settings')
  // Intentionally invoked for side effects: loads and persists shared TOC settings.
  useTocSettings()

  const fileState = useEditorStore((state) => state.openFiles.get(filePath)) as
    | EditorFileState
    | undefined

  const { updateContent, setViewMode, updateCursorPosition, updateScrollTop } =
    useEditorStore.getState()

  const handleChange = useCallback(
    (content: string) => {
      updateContent(filePath, content)
    },
    [filePath, updateContent]
  )

  const handleCursorChange = useCallback(
    (line: number, col: number) => {
      updateCursorPosition(filePath, line, col)
    },
    [filePath, updateCursorPosition]
  )

  const handleScrollChange = useCallback(
    (scrollTop: number) => {
      updateScrollTop(filePath, scrollTop)
    },
    [filePath, updateScrollTop]
  )

  const previewKind = getPreviewKind(filePath)

  const handleToggleViewMode = useCallback(() => {
    if (!fileState) return
    const renderedMode = fileState.language === 'markdown' ? 'markdown' : 'preview'
    setViewMode(filePath, fileState.viewMode === 'code' ? renderedMode : 'code')
  }, [filePath, fileState, setViewMode])

  if (!fileState) {
    return (
      <div className="w-full h-full flex items-center justify-center text-muted-foreground">
        {t('editor.loading')}
      </div>
    )
  }

  if (isDiskPreview(previewKind) && previewKind) {
    return (
      <FilePreview kind={previewKind} src={previewSourceUrl(filePath, fileState.lastModified)} />
    )
  }

  const isMarkdownFile = fileState.language === 'markdown'
  const isSvgFile = previewKind === 'svg'
  const isHtmlFile = previewKind === 'html'

  return (
    <div className="w-full h-full flex flex-col">
      {(isMarkdownFile || isSvgFile || isHtmlFile) && (
        <EditorToolbar
          viewMode={fileState.viewMode}
          onToggleViewMode={handleToggleViewMode}
          filePath={filePath}
          isMarkdown={isMarkdownFile}
        />
      )}
      <div className="flex-1 relative overflow-hidden">
        {isSvgFile && fileState.viewMode === 'preview' ? (
          <FilePreview kind="svg" src={svgDataUrl(fileState.content)} />
        ) : isHtmlFile && fileState.viewMode === 'preview' ? (
          <HtmlPreview
            src={previewSourceUrl(filePath, fileState.lastModified)}
            name={filePath.split(/[\\/]/).pop() || filePath}
            unsaved={fileState.isDirty}
          />
        ) : isMarkdownFile && fileState.viewMode === 'markdown' ? (
          <MarkdownEditor
            filePath={filePath}
            content={fileState.content}
            isVisible={isVisible}
            onChange={handleChange}
          />
        ) : (
          <CodeEditor
            filePath={filePath}
            content={fileState.content}
            language={fileState.language}
            isVisible={isVisible}
            initialCursorPosition={fileState.cursorPosition}
            initialScrollTop={fileState.scrollTop}
            onChange={handleChange}
            onCursorChange={handleCursorChange}
            onScrollChange={handleScrollChange}
          />
        )}
      </div>
    </div>
  )
}
