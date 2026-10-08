import { FolderOpen, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  reloadUserKeybindingSchemes,
  useApplyKeybindingScheme
} from '@/hooks/use-keyboard-shortcuts'
import { openerApi } from '@/lib/api'
import { DEFAULT_SCHEME_ID, loadKeybindingSchemeFiles } from '@/lib/keybinding-schemes'
import { isTauriContext } from '@/lib/tauri-runtime'
import { useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'

const SCHEME_FILE_EXAMPLE = `{
  "name": "My Keys",
  "extends": "iterm2",
  "bindings": { "splitRight": "cmd+d", "commandPalette": null }
}`

export function KeybindingSchemePicker(): React.JSX.Element {
  const { t } = useTranslation('settings')
  const schemeId = useKeyboardShortcutsStore((state) => state.schemeId)
  const schemes = useKeyboardShortcutsStore((state) => state.schemes)
  const schemeIssues = useKeyboardShortcutsStore((state) => state.schemeIssues)
  const applyScheme = useApplyKeybindingScheme()
  const canUseFiles = isTauriContext()

  const handleSelect = (nextId: string): void => {
    void applyScheme(nextId).catch((error: unknown) => {
      toast.error(error instanceof Error ? error.message : t('shortcuts.schemeSaveFailed'))
    })
  }

  const handleReload = async (): Promise<void> => {
    await reloadUserKeybindingSchemes()
    // Re-apply so edits to the active scheme's file take effect now.
    useKeyboardShortcutsStore.getState().applyScheme(useKeyboardShortcutsStore.getState().schemeId)
    toast.success(t('shortcuts.schemesReloaded'))
  }

  const handleOpenDir = async (): Promise<void> => {
    const result = await loadKeybindingSchemeFiles(true)
    if (!result.success) {
      toast.error(result.error)
      return
    }
    const opened = await openerApi.openWithExternalApp(result.data.dir)
    if (!opened.success) toast.error(opened.error)
  }

  return (
    <div className="space-y-2 rounded-md border border-border/70 bg-secondary/20 p-3">
      <label
        htmlFor="keybinding-scheme"
        className="block text-sm font-medium text-secondary-foreground"
      >
        {t('shortcuts.scheme')}
      </label>
      <div className="flex items-center gap-2">
        <select
          id="keybinding-scheme"
          value={schemeId}
          onChange={(event) => handleSelect(event.target.value)}
          className="h-8 flex-1 rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none transition-[border-color,background-color] duration-150 focus-visible:border-ring/70 focus-visible:bg-secondary/50 focus-visible:ring-1 focus-visible:ring-ring/35"
        >
          {schemes.map((scheme) => (
            <option key={scheme.id} value={scheme.id}>
              {scheme.id === DEFAULT_SCHEME_ID
                ? t('shortcuts.defaultScheme')
                : scheme.builtin
                  ? scheme.name
                  : t('shortcuts.userScheme', { name: scheme.name })}
            </option>
          ))}
        </select>
        {canUseFiles && (
          <>
            <button
              type="button"
              onClick={() => void handleReload()}
              className="inline-flex h-8 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
            >
              <RefreshCw size={12} />
              {t('shortcuts.reloadSchemes')}
            </button>
            <button
              type="button"
              onClick={() => void handleOpenDir()}
              className="inline-flex h-8 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
            >
              <FolderOpen size={12} />
              {t('shortcuts.openSchemeDir')}
            </button>
          </>
        )}
      </div>
      <p className="text-xs text-muted-foreground">{t('shortcuts.schemeHint')}</p>
      {canUseFiles && (
        <details className="text-xs text-muted-foreground">
          <summary className="cursor-pointer">{t('shortcuts.schemeFileHint')}</summary>
          <pre className="mt-1 overflow-x-auto rounded bg-secondary/50 p-2 font-mono text-2xs">
            {SCHEME_FILE_EXAMPLE}
          </pre>
        </details>
      )}
      {schemeIssues.length > 0 && (
        <ul className="space-y-0.5 text-xs text-amber-600 dark:text-amber-400">
          {schemeIssues.map((issue) => (
            <li key={`${issue.file}:${issue.message}`}>
              {issue.file ? `${issue.file}: ${issue.message}` : issue.message}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
