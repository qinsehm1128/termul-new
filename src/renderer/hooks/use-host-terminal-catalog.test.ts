import type { TerminalSpawnedEvent } from '@shared/types/ipc.types'
import { renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const onSpawned = vi.fn()
const adoptRemoteProjectTerminal = vi.fn()
const ensureTerminalTab = vi.fn()
const projectState = { activeProjectId: 'project-1' }

vi.mock('@/lib/log-api', () => ({ logFrontendError: vi.fn() }))
vi.mock('@/lib/terminal-api', () => ({
  terminalApi: {
    onSpawned: (callback: (event: TerminalSpawnedEvent) => void) => onSpawned(callback)
  }
}))
vi.mock('@/stores/project-store', () => ({
  useProjectStore: {
    getState: () => projectState
  }
}))
vi.mock('@/stores/terminal-store', () => ({
  useTerminalStore: {
    getState: () => ({ adoptRemoteProjectTerminal })
  }
}))
vi.mock('@/stores/workspace-store', () => ({
  useWorkspaceStore: {
    getState: () => ({ ensureTerminalTab })
  }
}))

import { useHostTerminalCatalog } from './use-host-terminal-catalog'

describe('useHostTerminalCatalog', () => {
  beforeEach(() => {
    projectState.activeProjectId = 'project-1'
    onSpawned.mockReset()
    adoptRemoteProjectTerminal.mockReset()
    ensureTerminalTab.mockReset()
  })

  it('adopts a phone-created project terminal without activating it', () => {
    let listener: ((event: TerminalSpawnedEvent) => void) | undefined
    onSpawned.mockImplementation((callback: (event: TerminalSpawnedEvent) => void) => {
      listener = callback
      return () => undefined
    })
    adoptRemoteProjectTerminal.mockReturnValue('pty-phone')

    renderHook(() => useHostTerminalCatalog())
    listener?.({
      terminalId: 'pty-phone',
      projectId: 'project-1',
      cwd: '/tmp/demo',
      cols: 80,
      rows: 24,
      shell: 'zsh'
    })

    expect(adoptRemoteProjectTerminal).toHaveBeenCalledTimes(1)
    expect(ensureTerminalTab).toHaveBeenCalledWith('pty-phone', undefined, false)
  })

  it('does not add a tab when the desktop is on another project', () => {
    projectState.activeProjectId = 'project-1'
    let listener: ((event: TerminalSpawnedEvent) => void) | undefined
    onSpawned.mockImplementation((callback: (event: TerminalSpawnedEvent) => void) => {
      listener = callback
      return () => undefined
    })
    adoptRemoteProjectTerminal.mockReturnValue('pty-other')

    renderHook(() => useHostTerminalCatalog())
    listener?.({
      terminalId: 'pty-other',
      projectId: 'project-2',
      cwd: '/tmp/other',
      cols: 80,
      rows: 24,
      shell: 'zsh'
    })

    expect(ensureTerminalTab).not.toHaveBeenCalled()
  })
})
