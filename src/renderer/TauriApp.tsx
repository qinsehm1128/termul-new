import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { useEffect } from 'react'
import { RouterProvider } from 'react-router-dom'
import { PortableAppEffects } from '@/app/PortableAppEffects'
import { createPortableRouter } from '@/app/portable-router'
import { ConversationHostStatus } from '@/components/conversation/ConversationHostStatus'
import { ConversationRecoveryPanel } from '@/components/conversation/ConversationRecoveryPanel'
import { ErrorBoundary } from '@/components/ErrorBoundary'
import { GlobalContextMenu } from '@/components/GlobalContextMenu'
import { Toaster as Sonner } from '@/components/ui/sonner'
import { Toaster } from '@/components/ui/toaster'
import { TooltipProvider } from '@/components/ui/tooltip'
import { WhatsNewModal } from '@/components/WhatsNewModal'
import { usePreventDevToolsShortcuts } from '@/hooks/use-prevent-devtools-shortcuts'
import { useWhatsNew } from '@/hooks/use-whats-new'
import { useWindowState } from '@/hooks/use-window-state'
import { getCurrentWindow } from '@/lib/tauri-window'

const queryClient = new QueryClient()
const router = createPortableRouter()

export default function TauriApp(): React.JSX.Element {
  usePreventDevToolsShortcuts()
  const isWindowStateReady = useWindowState()
  const whatsNew = useWhatsNew()

  useEffect(() => {
    if (!isWindowStateReady) return

    const showWindow = async (): Promise<void> => {
      if (typeof (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ === 'undefined')
        return
      try {
        await getCurrentWindow().show()
      } catch (error) {
        console.error('Failed to show window:', error)
      }
    }

    void showWindow()
  }, [isWindowStateReady])

  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <GlobalContextMenu>
          <ErrorBoundary context="appRoot">
            <PortableAppEffects />
            <ConversationHostStatus />
            <ConversationRecoveryPanel />
            <Toaster />
            <Sonner />
            <RouterProvider router={router} future={{ v7_startTransition: true }} />
            <WhatsNewModal
              isOpen={whatsNew.isOpen}
              version={whatsNew.version}
              notes={whatsNew.notes}
              htmlUrl={whatsNew.htmlUrl}
              onClose={whatsNew.close}
            />
          </ErrorBoundary>
        </GlobalContextMenu>
      </TooltipProvider>
    </QueryClientProvider>
  )
}
