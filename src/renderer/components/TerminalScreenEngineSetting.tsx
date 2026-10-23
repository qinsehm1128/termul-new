import type { TerminalScreenEngine } from '@shared/types/terminal-screen.types'
import { useId } from 'react'
import { useTranslation } from 'react-i18next'
import { isTauriContext } from '@/lib/tauri-runtime'

interface TerminalScreenEngineSettingProps {
  value: TerminalScreenEngine
  onChange: (value: TerminalScreenEngine) => void
}

/** Both renderer roots share the same host-owned project preference control. */
export function TerminalScreenEngineSetting({ value, onChange }: TerminalScreenEngineSettingProps) {
  const { t } = useTranslation('projects')
  const id = useId()
  const editable = isTauriContext()

  return (
    <div className="space-y-2">
      <label htmlFor={id} className="block text-sm font-medium text-secondary-foreground">
        {t('terminalScreenEngine')}
      </label>
      <select
        id={id}
        value={value}
        disabled={!editable}
        onChange={(event) =>
          onChange(event.target.value === 'ghostty-vt' ? 'ghostty-vt' : 'existing')
        }
        className="h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground disabled:opacity-60"
      >
        <option value="existing">{t('terminalScreenEngineExisting')}</option>
        <option value="ghostty-vt">{t('terminalScreenEngineGhostty')}</option>
      </select>
      <p className="text-xs text-muted-foreground">{t('terminalScreenEngineDescription')}</p>
      {!editable && (
        <p className="text-xs text-muted-foreground">{t('terminalScreenEngineReadOnly')}</p>
      )}
    </div>
  )
}
