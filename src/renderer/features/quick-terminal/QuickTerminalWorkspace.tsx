import type { QuickTerminalRecord } from '@shared/types/quick-terminal.types'
import { Plus, Terminal, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { flushEditorContent } from '@/lib/editor-content-flush'
import { releaseShownFile } from '@/lib/file-open-target'
import { logFrontendError } from '@/lib/log-api'
import { useEditorStore } from '@/stores/editor-store'
import { matchesShortcut, useKeyboardShortcutsStore } from '@/stores/keyboard-shortcuts-store'
import { QuickTerminalFilePreview } from './QuickTerminalFilePreview'
import { QuickTerminalView } from './QuickTerminalView'
import { claimQuickTabClose } from './quick-tab-close'
import { loadQuickEditorFile, useQuickEditorDraftPersistence } from './quick-terminal-editor-drafts'
import { failureMessage, useQuickTerminalStore } from './quick-terminal-store'
import { type QuickWorkspaceTab, useQuickWorkspaceStore } from './quick-terminal-workspace-store'

function warnEditor(operation: string): void {
  void logFrontendError({
    level: 'warn',
    source: 'quick-terminal-workspace',
    message: `operation=editor-${operation} code=EDITOR_FAILED`
  }).catch(() => undefined)
}

export function addQuickTerminalTab(id: string): void {
  const tabId = `new:${crypto.randomUUID()}`
  useQuickWorkspaceStore.getState().update(id, (layout) => ({
    ...layout,
    tabs: [...layout.tabs, { id: tabId, type: 'terminal', create: true }],
    activeTabId: tabId,
    lastTerminalTabId: tabId
  }))
}
export function openQuickEditorTab(id: string, path: string): void {
  const tabId = `editor:${path}`
  useQuickWorkspaceStore.getState().update(id, (layout) => ({
    ...layout,
    tabs: layout.tabs.some((tab) => tab.id === tabId)
      ? layout.tabs
      : [...layout.tabs, { id: tabId, type: 'editor', filePath: path }],
    activeTabId: tabId
  }))
}

export function QuickTerminalWorkspace({
  record
}: {
  record: QuickTerminalRecord
}): React.JSX.Element {
  const { t } = useTranslation('quickTerminal')
  const layout = useQuickWorkspaceStore((state) => state.layouts[record.id])
  const load = useQuickWorkspaceStore((state) => state.load)
  const update = useQuickWorkspaceStore((state) => state.update)
  const [closing, setClosing] = useState<string | null>(null)
  const dirtyKey = useEditorStore((state) =>
    [...state.openFiles.entries()]
      .filter(([, file]) => file.isDirty)
      .map(([path]) => path)
      .join('\n')
  )
  const dirty = new Set(dirtyKey.split('\n'))
  const activeTab = layout?.tabs.find((tab) => tab.id === layout.activeTabId)

  useQuickEditorDraftPersistence(record.id)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loadAttempt, setLoadAttempt] = useState(0)
  const initialRecord = useRef(record)
  useEffect(() => {
    void load(initialRecord.current)
  }, [load])
  const activePath = activeTab?.type === 'editor' ? activeTab.filePath : null
  // biome-ignore lint/correctness/useExhaustiveDependencies: loadAttempt retries the same file path
  useEffect(() => {
    let cancelled = false
    setLoadError(null)
    // Loading must not publish global ownership. A late disk read may finish after navigation.
    useEditorStore.getState().setActiveFilePath(null)
    if (activePath) {
      void loadQuickEditorFile(record.id, activePath)
        .then(() => {
          if (cancelled) return
          const current = useQuickWorkspaceStore.getState().layouts[record.id]
          const active = current?.tabs.find((tab) => tab.id === current.activeTabId)
          if (active?.type === 'editor' && active.filePath === activePath) {
            useEditorStore.getState().setActiveFilePath(activePath)
          }
        })
        .catch(() => {
          // Do not log file contents or backend error text (which can contain private paths).
          warnEditor('load')
          if (!cancelled) setLoadError(activePath)
        })
    }
    return () => {
      cancelled = true
    }
  }, [activePath, record.id, loadAttempt])
  useEffect(
    () => () => {
      const tabs = useQuickWorkspaceStore.getState().layouts[record.id]?.tabs ?? []
      for (const tab of tabs) {
        if (tab.type !== 'editor') continue
        const otherOwner = Object.entries(useQuickWorkspaceStore.getState().layouts).some(
          ([id, value]) =>
            id !== record.id &&
            value.tabs.some((item) => item.type === 'editor' && item.filePath === tab.filePath)
        )
        if (!otherOwner) {
          void flushEditorContent(tab.filePath)
            .then(() => releaseShownFile(tab.filePath))
            .catch(() => warnEditor('release'))
        }
      }
    },
    [record.id]
  )

  const activationRequest = useRef(0)
  const activate = async (tab: QuickWorkspaceTab): Promise<void> => {
    const request = ++activationRequest.current
    try {
      if (activePath) await flushEditorContent(activePath)
    } catch {
      warnEditor('switch')
      toast.error(t('fileBusy'))
      return
    }
    if (request !== activationRequest.current) return
    update(record.id, (previous) => ({
      ...previous,
      activeTabId: tab.id,
      lastTerminalTabId: tab.type === 'terminal' ? tab.id : previous.lastTerminalTabId
    }))
  }
  const closeTab = async (tab: QuickWorkspaceTab): Promise<void> => {
    if (closing) return
    if (tab.type === 'editor') {
      try {
        await flushEditorContent(tab.filePath)
      } catch {
        warnEditor('close')
        toast.error(t('fileBusy'))
        return
      }
      const file = useEditorStore.getState().openFiles.get(tab.filePath)
      if (
        file &&
        (file.isDirty || file.operationStatus === 'saving' || file.operationStatus === 'reloading')
      ) {
        toast.error(t('fileBusy'))
        return
      }
      // Respect project panes and other quick workspaces that also own this buffer.
      const otherOwner = Object.entries(useQuickWorkspaceStore.getState().layouts).some(
        ([id, value]) =>
          id !== record.id &&
          value.tabs.some((item) => item.type === 'editor' && item.filePath === tab.filePath)
      )
      if (!otherOwner) releaseShownFile(tab.filePath)
    } else if (tab.terminalId) {
      setClosing(tab.id)
      const result = await useQuickTerminalStore.getState().closeTerminal(record.id, tab.terminalId)
      setClosing(null)
      if (!result.success) {
        toast.error(failureMessage(result, t('closeFailed')))
        return
      }
    } else return // Do not discard a pending spawn; its result still belongs to this tab.
    update(record.id, (previous) => {
      const tabs = previous.tabs.filter((item) => item.id !== tab.id)
      const terminal =
        tabs.find((item) => item.id === previous.lastTerminalTabId && item.type === 'terminal') ??
        [...tabs].reverse().find((item) => item.type === 'terminal')
      return {
        tabs,
        lastTerminalTabId: terminal?.id ?? null,
        activeTabId:
          previous.activeTabId === tab.id || !tabs.some((item) => item.id === previous.activeTabId)
            ? (terminal?.id ?? tabs[0]?.id ?? null)
            : previous.activeTabId
      }
    })
  }
  const closeRef = useRef(closeTab)
  closeRef.current = closeTab
  useEffect(
    () =>
      claimQuickTabClose(() => {
        const current = useQuickWorkspaceStore.getState().layouts[record.id]
        const tab = current?.tabs.find((item) => item.id === current.activeTabId)
        if (tab) void closeRef.current(tab)
      }),
    [record.id]
  )
  useEffect(() => {
    const handler = (event: KeyboardEvent): void => {
      const shortcut = useKeyboardShortcutsStore.getState().shortcuts.closeTab
      if (!shortcut || !matchesShortcut(event, shortcut.customKey ?? shortcut.defaultKey)) return
      const current = useQuickWorkspaceStore.getState().layouts[record.id]
      const tab = current?.tabs.find((item) => item.id === current.activeTabId)
      event.preventDefault()
      event.stopImmediatePropagation()
      if (tab) void closeRef.current(tab)
    }
    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [record.id])
  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col">
      <div className="flex shrink-0 border-b">
        <div
          role="tablist"
          aria-label={t('workspaceTabs')}
          className="flex min-w-0 flex-1 overflow-x-auto"
        >
          {layout?.tabs.map((tab, index) => (
            <div
              key={tab.id}
              className="flex shrink-0 items-center border-r"
              data-active={tab.id === layout.activeTabId}
            >
              <button
                type="button"
                role="tab"
                aria-selected={tab.id === layout.activeTabId}
                onClick={() => activate(tab)}
                className={`flex h-9 max-w-56 items-center gap-2 px-3 text-xs ${tab.id === layout.activeTabId ? 'bg-accent text-foreground' : 'text-muted-foreground'}`}
                title={tab.type === 'editor' ? tab.filePath : undefined}
              >
                {tab.type === 'terminal' ? (
                  <Terminal className="size-3.5" aria-hidden="true" />
                ) : null}
                <span className="truncate">
                  {tab.type === 'editor'
                    ? `${tab.filePath.split(/[\\/]/).pop()}${dirty.has(tab.filePath) ? ' •' : ''}`
                    : t('terminalTab', { number: String(index + 1) })}
                </span>
              </button>
              <Button
                size="icon"
                variant="ghost"
                className="mr-1 size-6"
                aria-label={tab.type === 'editor' ? t('closePreview') : t('closeTerminalTab')}
                disabled={closing === tab.id || (tab.type === 'terminal' && !tab.terminalId)}
                onClick={() => void closeTab(tab)}
              >
                <X className="size-3" aria-hidden="true" />
              </Button>
            </div>
          ))}
        </div>
        <Button
          size="icon"
          variant="ghost"
          className="h-9 w-9 shrink-0"
          aria-label={t('newTerminalTab')}
          disabled={!layout}
          onClick={() => addQuickTerminalTab(record.id)}
        >
          <Plus className="size-4" aria-hidden="true" />
        </Button>
      </div>
      <div className="relative min-h-0 flex-1">
        {layout?.tabs
          .filter((tab) => tab.type === 'terminal')
          .map((tab) =>
            tab.type === 'terminal' ? (
              <div
                key={tab.id}
                role="tabpanel"
                aria-hidden={tab.id !== layout.activeTabId}
                className={`absolute inset-0 ${tab.id === layout.activeTabId ? '' : 'invisible pointer-events-none'}`}
              >
                <QuickTerminalView
                  record={record}
                  tabId={tab.id}
                  terminalId={tab.terminalId}
                  createNew={tab.create}
                  isVisible={tab.id === layout.activeTabId}
                  onOpened={(terminalId) =>
                    update(record.id, (previous) => ({
                      ...previous,
                      tabs: previous.tabs
                        .filter(
                          (item) =>
                            item.id === tab.id ||
                            item.type !== 'terminal' ||
                            item.terminalId !== terminalId
                        )
                        .map((item) =>
                          item.id === tab.id ? { id: item.id, type: 'terminal', terminalId } : item
                        )
                    }))
                  }
                />
              </div>
            ) : null
          )}
        {activeTab?.type === 'editor' ? (
          loadError === activeTab.filePath ? (
            <section
              role="alert"
              className="flex h-full flex-col items-center justify-center gap-3 p-6"
            >
              <p>{t('fileLoadFailed')}</p>
              <div className="flex gap-2">
                <Button variant="outline" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>
                  {t('retryFile')}
                </Button>
                <Button variant="outline" onClick={() => void closeTab(activeTab)}>
                  {t('closePreview')}
                </Button>
              </div>
            </section>
          ) : (
            <QuickTerminalFilePreview key={activeTab.filePath} path={activeTab.filePath} />
          )
        ) : null}
        {layout && !activeTab ? (
          <div className="flex h-full items-center justify-center">
            <Button variant="outline" onClick={() => addQuickTerminalTab(record.id)}>
              {t('reopen')}
            </Button>
          </div>
        ) : null}
      </div>
    </div>
  )
}
