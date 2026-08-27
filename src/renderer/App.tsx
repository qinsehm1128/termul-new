import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { useEffect } from 'react'
import { RouterProvider } from 'react-router-dom'
import { PortableAppEffects } from '@/app/PortableAppEffects'
import { createPortableRouter } from '@/app/portable-router'
import { ConversationHostStatus } from '@/components/conversation/ConversationHostStatus'
import { ConversationRecoveryPanel } from '@/components/conversation/ConversationRecoveryPanel'
import { DirectoryPicker } from '@/components/DirectoryPicker'
import { ErrorBoundary } from '@/components/ErrorBoundary'
import { GlobalContextMenu } from '@/components/GlobalContextMenu'
import { Toaster as Sonner } from '@/components/ui/sonner'
import { Toaster } from '@/components/ui/toaster'
import { TooltipProvider } from '@/components/ui/tooltip'
import { WhatsNewModal } from '@/components/WhatsNewModal'
import { useWhatsNew } from '@/hooks/use-whats-new'
import { isWindows } from '@/lib/platform'
import { isTauriContext } from '@/lib/tauri-runtime'

// PRODUCTION GUARDRAIL: This branch targets xterm 6.1-beta (the line VS Code
// ships in production). The 6.1 beta track includes memory leak fixes
// (IntersectionObserver retention, dispose-registration gaps) and TUI stability
// (alt-buffer teleport fix, currentRow OOM fix) not present in 6.0 stable.
// WebGL is preserved as the GPU renderer with DOM fallback ("canvas" removed in 6.0).
// See _bmad-output/implementation-artifacts/spec-gh133-xterm-6-1-upgrade-memory-leak-fix.md.

// Browser-only defense against Windows exposing its default menu bar. macOS
// keeps Alt/Option available for typing special characters.
function usePreventAltMenu(): void {
  useEffect(() => {
    if (!isWindows) return

    const preventAltMenu = (event: KeyboardEvent): void => {
      if (event.key === 'Alt') {
        event.preventDefault()
        event.stopPropagation()
      }
    }

    window.addEventListener('keydown', preventAltMenu, { capture: true })
    window.addEventListener('keyup', preventAltMenu, { capture: true })

    return () => {
      window.removeEventListener('keydown', preventAltMenu, { capture: true })
      window.removeEventListener('keyup', preventAltMenu, { capture: true })
    }
  }, [])
}

const queryClient = new QueryClient()
const router = createPortableRouter()

const App = () => {
  usePreventAltMenu()
  const whatsNew = useWhatsNew()

  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider delayDuration={80} skipDelayDuration={300}>
        <GlobalContextMenu>
          <ErrorBoundary context="appRoot">
            <PortableAppEffects />
            <ConversationHostStatus />
            <ConversationRecoveryPanel />
            <Toaster />
            <Sonner />
            {/* Browser/remote mode provides an in-app picker instead of the native dialog. */}
            {!isTauriContext() && <DirectoryPicker />}
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

export default App
