import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { clipboardApi } from '@/lib/clipboard-api'
import { logFrontendError } from '@/lib/log-api'
import { isMac } from '@/lib/platform'
import { openerApi } from '@/lib/tauri-opener-api'
import { isTauriContext } from '@/lib/tauri-runtime'

/** Reveal the original on desktop. A browser must not open the host's file manager. */
export function FileLocationButton({ filePath }: { filePath: string }): React.JSX.Element {
  const { t } = useTranslation('settings')
  const desktop = isTauriContext()
  const label = desktop
    ? t(isMac ? 'editor.showInFinder' : 'editor.showInFileManager')
    : t('editor.copyFilePath')

  async function locate(): Promise<void> {
    const operation = desktop ? 'reveal' : 'copy_path'
    try {
      const result = desktop
        ? await openerApi.revealInFileManager(filePath)
        : await clipboardApi.writeText(filePath)
      if (result.success) {
        if (!desktop) toast.success(t('editor.filePathCopied'))
        return
      }
    } catch {
      // Native and clipboard errors may include a private path. Keep them out of logs.
    }
    void logFrontendError({
      level: 'warn',
      source: 'editor.file-location',
      message: `operation=${operation} code=FILE_LOCATION_FAILED`
    })
    toast.error(t(desktop ? 'editor.revealFileFailed' : 'editor.copyFilePathFailed'))
  }

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-sm"
      title={label}
      aria-label={label}
      onClick={() => void locate()}
    >
      <svg
        aria-hidden="true"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        {desktop ? (
          <path d="M2 12.5V4a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1v1H5l-3 6Zm0 0h10l2-6" />
        ) : (
          <path d="M5.5 11H3a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1h7a1 1 0 0 1 1 1v2.5M7 5.5h6a1 1 0 0 1 1 1V13a1 1 0 0 1-1 1H6.5a1 1 0 0 1-1-1V6.5a1 1 0 0 1 1-1Z" />
        )}
      </svg>
    </Button>
  )
}
