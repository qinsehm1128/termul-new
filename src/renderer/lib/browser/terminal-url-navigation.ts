import { acceptedBrandValues } from '@shared/brand'
import { openerApi } from '@/lib/api'
import { randomUUID } from '@/lib/uuid'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { useBrowserSessionStore } from '@/stores/browser-session-store'
import { useWorkspaceStore } from '@/stores/workspace-store'

export const TERMINAL_DEDICATED_BROWSER_TAB_ID = 'terminal-link-browser'

function createTerminalBrowserTabId(): string {
  return `${TERMINAL_DEDICATED_BROWSER_TAB_ID}-${randomUUID()}`
}

export async function openTerminalUrlInDedicatedBrowser(url: string): Promise<void> {
  const targetTabId = createTerminalBrowserTabId()

  useBrowserSessionStore.getState().ensureTab(targetTabId, url)
  useWorkspaceStore.getState().addBrowserTab(targetTabId)
}

export async function openTerminalUrl(url: string): Promise<void> {
  const { terminalUrlOpenMode } = useAppSettingsStore.getState().settings

  // Membership, not equality: settings persisted before the rename carry the
  // legacy enum member, and a user who chose the built-in browser must not be
  // silently moved back to the system one.
  if (acceptedBrandValues('urlOpenMode').includes(terminalUrlOpenMode)) {
    await openTerminalUrlInDedicatedBrowser(url)
    return
  }

  const result = await openerApi.openUrlWithSystemBrowser(url)
  if (!result.success) {
    throw new Error(result.error || 'Failed to open URL in system browser')
  }
}
