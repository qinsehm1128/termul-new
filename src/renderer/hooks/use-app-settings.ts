import { brandCanonical, LEGACY } from '@shared/brand'
import { useCallback, useEffect } from 'react'
import { runtimeT } from '@/i18n/runtime'
import { acpApi, persistenceApi, terminalApi } from '@/lib/api'
import { getSystemAppearance, normalizeThemeFamilyId } from '@/lib/themes/theme-appearance'
import { useAppSettingsStore } from '@/stores/app-settings-store'
import { useCliSessionPanelStore } from '@/stores/cli-session-panel-store'
import { useFileExplorerStore } from '@/stores/file-explorer-store'
import { useSidebarStore } from '@/stores/sidebar-store'
import { useSSHPanelStore } from '@/stores/ssh-panel-store'
import { useTerminalListPanelStore } from '@/stores/terminal-list-panel-store'
import type { AppPanelVisibilitySettingKey, AppSettings, AppSettingsUpdate } from '@/types/settings'
import { APP_SETTINGS_KEY, DEFAULT_APP_SETTINGS, isUiLanguagePreference } from '@/types/settings'

type PanelSettingKey = AppPanelVisibilitySettingKey

type PanelWriteRequest = {
  panel: PanelSettingKey
  visible: boolean
  requestId: number
  revision: number
}

let panelWriteChain: Promise<void> = Promise.resolve()
/**
 * Per-panel write sequence numbers, and — because `Record<PanelSettingKey, _>`
 * forces every key to be present — the one exhaustive list of panel keys.
 * Everything below derives its key set from here, so adding a panel is a
 * single edit plus whatever compile errors TypeScript raises.
 */
const panelWriteRequestIds: Record<PanelSettingKey, number> = {
  sidebarVisible: 0,
  fileExplorerVisible: 0,
  sshPanelVisible: 0,
  cliSessionPanelVisible: 0,
  terminalListPanelVisible: 0
}

const PANEL_VISIBILITY_KEYS = Object.keys(panelWriteRequestIds) as PanelSettingKey[]
let panelWriteRevision = 0
let lastSuccessfulPanelWriteRevision = 0
let persistedPanelSettingsSnapshot: AppSettings = { ...DEFAULT_APP_SETTINGS }
let pendingPanelWriteCount = 0
let pendingPanelWriteWaiters: Array<() => void> = []

function notifyPanelWriteSettled(): void {
  if (pendingPanelWriteCount === 0 && pendingPanelWriteWaiters.length > 0) {
    const waiters = pendingPanelWriteWaiters
    pendingPanelWriteWaiters = []
    waiters.forEach((resolve) => {
      resolve()
    })
  }
}

function syncPersistedPanelSettingsSnapshot(settings: AppSettings): void {
  persistedPanelSettingsSnapshot = { ...settings }
}

function buildPanelWriteSnapshot(request: PanelWriteRequest): AppSettings {
  // Every panel field is rebuilt from the last *persisted* values so this
  // write cannot smuggle another panel's optimistic, not-yet-written store
  // value into the payload.
  const snapshot: AppSettings = { ...useAppSettingsStore.getState().settings }
  for (const key of PANEL_VISIBILITY_KEYS) {
    snapshot[key] = persistedPanelSettingsSnapshot[key]
  }
  snapshot[request.panel] = request.visible
  return snapshot
}

function enqueuePanelWrite(request: PanelWriteRequest): Promise<void> {
  pendingPanelWriteCount += 1
  const run = panelWriteChain.then(async () => {
    const settingsSnapshot = buildPanelWriteSnapshot(request)
    const result = await persistenceApi.write(APP_SETTINGS_KEY, settingsSnapshot)

    if (!result.success) {
      const isLatestPanelRequest = panelWriteRequestIds[request.panel] === request.requestId
      const canRollbackToLastPersistedValue = request.revision > lastSuccessfulPanelWriteRevision

      if (isLatestPanelRequest && canRollbackToLastPersistedValue) {
        const rollbackValue = persistedPanelSettingsSnapshot[request.panel]
        useAppSettingsStore.getState().updateSetting(request.panel, rollbackValue)
        applyPanelVisibilityToUi(request.panel, rollbackValue)
      }

      throw new Error(
        result.error ||
          runtimeT('common', 'persistence.failedPanel', 'Failed to persist {{panel}}', {
            panel: request.panel
          })
      )
    }

    syncPersistedPanelSettingsSnapshot(settingsSnapshot)
    lastSuccessfulPanelWriteRevision = request.revision
  })
  panelWriteChain = run.catch(() => undefined)

  return run.finally(() => {
    pendingPanelWriteCount = Math.max(0, pendingPanelWriteCount - 1)
    notifyPanelWriteSettled()
  })
}

export async function waitForPendingAppSettingsPersistence(): Promise<void> {
  await panelWriteChain.catch(() => undefined)

  if (pendingPanelWriteCount === 0) {
    return
  }

  await new Promise<void>((resolve) => {
    pendingPanelWriteWaiters.push(resolve)
  })
}

export function resetAppSettingsPersistenceQueueForTests(): void {
  panelWriteChain = Promise.resolve()
  for (const key of PANEL_VISIBILITY_KEYS) {
    panelWriteRequestIds[key] = 0
  }
  panelWriteRevision = 0
  lastSuccessfulPanelWriteRevision = 0
  persistedPanelSettingsSnapshot = { ...DEFAULT_APP_SETTINGS }
  pendingPanelWriteCount = 0
  pendingPanelWriteWaiters = []
}

/**
 * Exhaustive on purpose. This used to fall through to the file explorer for
 * any unrecognised key, so a panel added without a branch here would silently
 * toggle a different panel. The `never` assignment turns that into a compile
 * error instead.
 */
function applyPanelVisibilityToUi(panel: PanelSettingKey, visible: boolean): void {
  switch (panel) {
    case 'sidebarVisible':
      useSidebarStore.getState().setVisible(visible)
      return
    case 'sshPanelVisible':
      useSSHPanelStore.getState().setVisible(visible)
      return
    case 'cliSessionPanelVisible':
      useCliSessionPanelStore.getState().setVisible(visible)
      return
    case 'terminalListPanelVisible':
      useTerminalListPanelStore.getState().setVisible(visible)
      return
    case 'fileExplorerVisible':
      useFileExplorerStore.getState().setVisible(visible)
      return
    default: {
      const unhandled: never = panel
      throw new Error(`Unhandled panel visibility key: ${String(unhandled)}`)
    }
  }
}

export function useAppSettingsLoader(): void {
  const setSettings = useAppSettingsStore((state) => state.setSettings)

  useEffect(() => {
    async function load(): Promise<void> {
      const result = await persistenceApi.read<AppSettings>(APP_SETTINGS_KEY)
      let settings: AppSettings

      if (result.success && result.data) {
        // Merge with defaults to handle any missing keys from older versions
        settings = { ...DEFAULT_APP_SETTINGS, ...result.data }
        let shouldPersistSettings = false
        const rawAppearance = result.data.appearanceMode as string | undefined
        const hasLegacyLightThemeId = settings.colorTheme.endsWith('-light')

        if (hasLegacyLightThemeId) {
          settings = {
            ...settings,
            colorTheme: normalizeThemeFamilyId(settings.colorTheme),
            appearanceMode: settings.appearanceMode ?? 'light'
          }
          shouldPersistSettings = true
        }

        if (rawAppearance === undefined && !hasLegacyLightThemeId) {
          settings = { ...settings, appearanceMode: 'dark' }
          shouldPersistSettings = true
        }

        if (rawAppearance === 'system') {
          settings = { ...settings, appearanceMode: getSystemAppearance() }
          shouldPersistSettings = true
        } else if (settings.appearanceMode !== 'light' && settings.appearanceMode !== 'dark') {
          settings = { ...settings, appearanceMode: 'dark' }
          shouldPersistSettings = true
        }

        if (!isUiLanguagePreference(result.data.uiLanguage)) {
          settings = { ...settings, uiLanguage: 'system' }
          shouldPersistSettings = true
        }

        // Migrate persisted "canvas" renderer preference to "dom"
        // xterm 6.0 removed @xterm/addon-canvas; DOM is now the built-in fallback
        if ((settings as unknown as Record<string, unknown>).terminalRenderer === 'canvas') {
          settings = { ...settings, terminalRenderer: 'dom' as const }
          shouldPersistSettings = true
        }

        if (shouldPersistSettings) {
          void persistenceApi.writeDebounced(APP_SETTINGS_KEY, settings)
        }

        // A settings blob written before the rename still names the brand
        // theme by its legacy id. Normalize once, here at the read boundary,
        // so nothing downstream has to know two spellings. Deliberately after
        // the persist above and idempotent on the next load: this is a
        // compatibility read, and letting it ride into a write another branch
        // already scheduled would flip a persisted value a wave early.
        if (settings.colorTheme === LEGACY.themeId) {
          settings = { ...settings, colorTheme: brandCanonical().themeId }
        }

        setSettings(settings)
      } else {
        settings = DEFAULT_APP_SETTINGS
        setSettings(settings)
      }

      syncPersistedPanelSettingsSnapshot(settings)

      useSidebarStore.getState().setVisible(settings.sidebarVisible)
      useFileExplorerStore.getState().setVisible(settings.fileExplorerVisible)
      useSSHPanelStore.getState().setVisible(settings.sshPanelVisible)
      useCliSessionPanelStore.getState().setVisible(settings.cliSessionPanelVisible)
      useTerminalListPanelStore.getState().setVisible(settings.terminalListPanelVisible)

      // Apply orphan detection settings to PtyManager after settings load
      try {
        await terminalApi.updateOrphanDetection(
          settings.orphanDetectionEnabled,
          settings.orphanDetectionTimeout
        )
      } catch (error) {
        console.error('Failed to apply orphan detection settings:', error)
      }

      // Push the ACP timeout overrides to the Rust core (desktop-only via the
      // transport; the WS transport no-ops on the standalone server, which
      // configures via the TERMUL_ACP_* env vars).
      try {
        await acpApi.setTurnTimeout(settings.acpTurnTimeoutSecs)
      } catch (error) {
        console.error('Failed to apply ACP turn timeout:', error)
      }
      try {
        await acpApi.setTurnIdleTimeout(settings.acpTurnIdleTimeoutSecs)
      } catch (error) {
        console.error('Failed to apply ACP turn idle timeout:', error)
      }
      try {
        await acpApi.setSessionNewTimeout(settings.acpSessionNewTimeoutSecs)
      } catch (error) {
        console.error('Failed to apply ACP session/new timeout:', error)
      }
      try {
        await acpApi.setSessionReopenTimeout(settings.acpSessionReopenTimeoutSecs)
      } catch (error) {
        console.error('Failed to apply ACP session reopen timeout:', error)
      }
      try {
        await acpApi.setFirstPromptWarmupTimeout(settings.acpFirstPromptWarmupSecs)
      } catch (error) {
        console.error('Failed to apply ACP first-prompt warmup timeout:', error)
      }
      try {
        await acpApi.setPreferLocalNpmInstall(settings.acpPreferLocalNpmInstall)
      } catch (error) {
        console.error('Failed to apply ACP local npm install preference:', error)
      }
    }
    load()
  }, [setSettings])
}

export function useUpdateAppSetting<K extends keyof AppSettings>(): (
  key: K,
  value: AppSettings[K]
) => Promise<void> {
  const updateSetting = useAppSettingsStore((state) => state.updateSetting)

  return useCallback(
    async (key: K, value: AppSettings[K]) => {
      updateSetting(key, value)
      // Use callback to get the latest state after update
      // Note: Zustand updates are synchronous, so getState() after updateSetting() returns updated state
      const updatedSettings = useAppSettingsStore.getState().settings
      await persistenceApi.writeDebounced(APP_SETTINGS_KEY, updatedSettings)
    },
    [updateSetting]
  )
}

export function useUpdateAppSettings(): (updates: AppSettingsUpdate) => Promise<void> {
  const updateSettings = useAppSettingsStore((state) => state.updateSettings)

  return useCallback(
    async (updates: AppSettingsUpdate) => {
      updateSettings(updates)
      const updatedSettings = useAppSettingsStore.getState().settings
      await persistenceApi.writeDebounced(APP_SETTINGS_KEY, updatedSettings)
    },
    [updateSettings]
  )
}

export function useUpdatePanelVisibility(): (
  panel: PanelSettingKey,
  visible: boolean
) => Promise<void> {
  const updateSetting = useAppSettingsStore((state) => state.updateSetting)

  return useCallback(
    async (panel: PanelSettingKey, visible: boolean) => {
      const requestId = ++panelWriteRequestIds[panel]
      const request: PanelWriteRequest = {
        panel,
        visible,
        requestId,
        revision: ++panelWriteRevision
      }

      updateSetting(panel, visible)
      applyPanelVisibilityToUi(panel, visible)

      return enqueuePanelWrite(request)
    },
    [updateSetting]
  )
}

export function useResetAppSettings(): () => Promise<void> {
  const resetToDefaults = useAppSettingsStore((state) => state.resetToDefaults)

  return useCallback(async () => {
    resetToDefaults()
    useSidebarStore.getState().setVisible(DEFAULT_APP_SETTINGS.sidebarVisible)
    useFileExplorerStore.getState().setVisible(DEFAULT_APP_SETTINGS.fileExplorerVisible)
    useSSHPanelStore.getState().setVisible(DEFAULT_APP_SETTINGS.sshPanelVisible)
    useCliSessionPanelStore.getState().setVisible(DEFAULT_APP_SETTINGS.cliSessionPanelVisible)
    useTerminalListPanelStore.getState().setVisible(DEFAULT_APP_SETTINGS.terminalListPanelVisible)

    const result = await persistenceApi.write(APP_SETTINGS_KEY, DEFAULT_APP_SETTINGS)
    if (result.success) {
      syncPersistedPanelSettingsSnapshot(DEFAULT_APP_SETTINGS)
    }
    // Clear the in-process ACP timeout overrides too (mirrors the load
    // hook's push, so a reset doesn't leave stale overrides in the Rust core).
    try {
      await acpApi.setTurnTimeout(DEFAULT_APP_SETTINGS.acpTurnTimeoutSecs)
    } catch (error) {
      console.error('Failed to clear ACP turn timeout on reset:', error)
    }
    try {
      await acpApi.setTurnIdleTimeout(DEFAULT_APP_SETTINGS.acpTurnIdleTimeoutSecs)
    } catch (error) {
      console.error('Failed to clear ACP turn idle timeout on reset:', error)
    }
    try {
      await acpApi.setSessionNewTimeout(DEFAULT_APP_SETTINGS.acpSessionNewTimeoutSecs)
    } catch (error) {
      console.error('Failed to clear ACP session/new timeout on reset:', error)
    }
    try {
      await acpApi.setSessionReopenTimeout(DEFAULT_APP_SETTINGS.acpSessionReopenTimeoutSecs)
    } catch (error) {
      console.error('Failed to clear ACP session reopen timeout on reset:', error)
    }
    try {
      await acpApi.setFirstPromptWarmupTimeout(DEFAULT_APP_SETTINGS.acpFirstPromptWarmupSecs)
    } catch (error) {
      console.error('Failed to clear ACP first-prompt warmup timeout on reset:', error)
    }
    try {
      await acpApi.setPreferLocalNpmInstall(DEFAULT_APP_SETTINGS.acpPreferLocalNpmInstall)
    } catch (error) {
      console.error('Failed to reset ACP local npm install preference:', error)
    }
  }, [resetToDefaults])
}
