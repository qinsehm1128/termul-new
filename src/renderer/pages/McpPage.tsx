import { Boxes, Cable, FileText, Plug, Server } from 'lucide-react'
import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { McpCapabilitiesPanel, McpUpstreamsPanel } from '@/components/settings/McpControlPanel'
import { McpDescriptionsPanel } from '@/components/settings/McpDescriptionsPanel'
import { McpGatewayPanel } from '@/components/settings/McpGatewayPanel'
import { PageTabs, PageTabsContent, PageTabsList, PageTabsTrigger } from '@/components/ui/page-tabs'
import { isTauriContext } from '@/lib/tauri-runtime'
import { useMcpStore } from '@/stores/mcp-store'

export default function McpPage(): React.JSX.Element {
  const { t } = useTranslation('mcp')
  const load = useMcpStore((state) => state.load)
  // The gateway and the server descriptions live on the desktop host only.
  const desktop = isTauriContext()

  useEffect(() => {
    void load()
  }, [load])

  return (
    <div className="flex h-full flex-col overflow-auto bg-background">
      <header className="border-b border-border px-4 py-4 sm:px-6">
        <div className="flex items-center gap-2">
          <Plug size={16} className="text-primary" />
          <h1 className="text-lg font-medium text-foreground">{t('page.title')}</h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">{t('page.subtitle')}</p>
      </header>
      <div className="mx-auto w-full max-w-5xl px-4 py-5 sm:px-6">
        <PageTabs defaultValue={desktop ? 'gateway' : 'servers'}>
          <PageTabsList aria-label={t('page.tabs.label')}>
            {desktop ? (
              <PageTabsTrigger value="gateway" icon={<Cable aria-hidden="true" />}>
                {t('page.tabs.gateway')}
              </PageTabsTrigger>
            ) : null}
            <PageTabsTrigger value="servers" icon={<Server aria-hidden="true" />}>
              {t('page.tabs.servers')}
            </PageTabsTrigger>
            <PageTabsTrigger value="capabilities" icon={<Boxes aria-hidden="true" />}>
              {t('page.tabs.capabilities')}
            </PageTabsTrigger>
            {desktop ? (
              <PageTabsTrigger value="descriptions" icon={<FileText aria-hidden="true" />}>
                {t('page.tabs.descriptions')}
              </PageTabsTrigger>
            ) : null}
          </PageTabsList>
          {desktop ? (
            <PageTabsContent value="gateway">
              <McpGatewayPanel />
            </PageTabsContent>
          ) : null}
          <PageTabsContent value="servers">
            <McpUpstreamsPanel />
          </PageTabsContent>
          <PageTabsContent value="capabilities">
            <McpCapabilitiesPanel />
          </PageTabsContent>
          {desktop ? (
            <PageTabsContent value="descriptions">
              <McpDescriptionsPanel />
            </PageTabsContent>
          ) : null}
        </PageTabs>
      </div>
    </div>
  )
}
