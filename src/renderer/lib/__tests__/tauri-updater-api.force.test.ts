import type { PendingUpdatePlan } from '@shared/types/updater.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockInvoke, mockSavePlan } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  mockSavePlan: vi.fn()
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(async () => '0.14.8') }))
vi.mock('@tauri-apps/plugin-updater', () => ({ check: vi.fn(), Update: class {} }))
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: vi.fn() }))
vi.mock('../tauri-backup-api', () => ({
  BackupErrorCodes: {},
  createBackup: vi.fn(async () => ({ success: true, data: { id: 'b' } })),
  setAppVersion: vi.fn()
}))
vi.mock('../tauri-rollback-api', () => ({
  keepPreviousVersion: vi.fn(async () => ({ success: true, data: {} })),
  setCurrentVersion: vi.fn()
}))
vi.mock('../tauri-update-plan-api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../tauri-update-plan-api')>()),
  savePendingUpdatePlan: mockSavePlan
}))

import { check } from '@tauri-apps/plugin-updater'
import {
  _resetUpdaterStateForTesting,
  checkForUpdates,
  downloadUpdate,
  installAndRestart
} from '../tauri-updater-api'

const declaredPolicy = {
  schemaVersion: 1,
  targetVersion: '0.14.9',
  metadataState: 'declared',
  components: {
    renderer: { buildId: 'renderer-next', action: 'restart' },
    guiNative: { buildId: 'gui-next', action: 'restart' },
    acpCore: { buildId: 'acp-next', action: 'defer-if-active' },
    terminalCore: { buildId: 'terminal-next', action: 'defer-if-active' }
  }
}

async function downloadDeclaredUpdate(): Promise<void> {
  mockInvoke.mockImplementation(async (command: string) =>
    command === 'updater_fetch_channel_manifest'
      ? { success: true, data: { version: '0.14.9', termul: { componentPolicy: declaredPolicy } } }
      : undefined
  )
  vi.mocked(check).mockResolvedValue({
    version: '0.14.9',
    download: vi.fn(async () => {}),
    install: vi.fn(async () => {})
  } as never)
  await checkForUpdates()
  await downloadUpdate()
}

function savedPlan(): PendingUpdatePlan {
  return mockSavePlan.mock.calls[0][0] as PendingUpdatePlan
}

describe('installAndRestart force option', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    _resetUpdaterStateForTesting()
    mockSavePlan.mockResolvedValue({ success: true, data: undefined })
  })

  it('keeps the published defer-if-active Core actions for a normal install', async () => {
    await downloadDeclaredUpdate()

    await installAndRestart()

    expect(savedPlan().componentPolicy.components.terminalCore.action).toBe('defer-if-active')
    expect(savedPlan().componentPolicy.components.acpCore.action).toBe('defer-if-active')
  })

  it('persists a plan that restarts both Cores for a forced install', async () => {
    await downloadDeclaredUpdate()

    await installAndRestart({ force: true })

    const { components } = savedPlan().componentPolicy
    expect(components.terminalCore).toEqual({ buildId: 'terminal-next', action: 'restart' })
    expect(components.acpCore).toEqual({ buildId: 'acp-next', action: 'restart' })
  })
})
