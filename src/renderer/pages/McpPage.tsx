import { Network } from 'lucide-react'
import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { McpControlPanel } from '@/components/settings/McpControlPanel'
import { useMcpStore } from '@/stores/mcp-store'

export default function McpPage(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const load = useMcpStore((state) => state.load)

  useEffect(() => {
    void load()
  }, [load])

  return (
    <div className="flex h-full flex-col overflow-auto bg-background">
      <header className="border-b border-border px-6 py-4">
        <div className="flex items-center gap-2">
          <Network size={16} className="text-primary" />
          <h1 className="text-lg font-medium text-foreground">{t('page.title')}</h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">{t('page.subtitle')}</p>
      </header>
      <div className="mx-auto w-full max-w-3xl space-y-6 p-6">
        <McpControlPanel />
      </div>
    </div>
  )
}
