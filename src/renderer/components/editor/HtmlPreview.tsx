import { useTranslation } from 'react-i18next'

interface HtmlPreviewProps {
  /** Asset URL of the saved file, so relative stylesheets, images and scripts resolve. */
  src: string
  name: string
  /** The buffer has edits the saved file does not have yet. */
  unsaved: boolean
}

/**
 * Renders an HTML file as a page. The sandbox runs its scripts (charts and
 * the like) but withholds `allow-same-origin`: the frame gets an opaque origin,
 * so it cannot reach the app, its storage or IPC, and the asset protocol's
 * CORS answer keeps it from reading other local files.
 */
export function HtmlPreview({ src, name, unsaved }: HtmlPreviewProps): React.JSX.Element {
  const { t } = useTranslation('settings')

  return (
    <div className="flex h-full w-full flex-col">
      {unsaved ? (
        <p className="shrink-0 border-b border-border/70 px-3 py-1.5 text-2xs text-muted-foreground">
          {t('editor.htmlPreviewUnsaved')}
        </p>
      ) : null}
      <iframe
        title={t('editor.htmlPreviewTitle', { name })}
        src={src}
        sandbox="allow-scripts"
        className="min-h-0 w-full flex-1 border-0 bg-white"
      />
    </div>
  )
}
