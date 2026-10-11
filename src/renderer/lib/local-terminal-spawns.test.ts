import type { IpcResult, SpawnedTerminal } from '@shared/types/ipc.types'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type Module = typeof import('./local-terminal-spawns')

function spawned(id: string): IpcResult<SpawnedTerminal> {
  return { success: true, data: { id } as SpawnedTerminal }
}

describe('local terminal spawns', () => {
  let mod: Module

  beforeEach(async () => {
    vi.resetModules()
    mod = await import('./local-terminal-spawns')
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('treats a spawn event as local while a local spawn awaits its reply', async () => {
    let reply: (result: IpcResult<SpawnedTerminal>) => void = () => undefined
    const pending = mod.trackLocalTerminalSpawn(
      () =>
        new Promise((resolve) => {
          reply = resolve
        })
    )

    // The host announces the spawn before the caller hears back.
    expect(mod.isLocalTerminalSpawnEvent('pty-1')).toBe(true)

    reply(spawned('pty-1'))
    await pending
  })

  it('claims the late event of a finished spawn once, by id', async () => {
    await mod.trackLocalTerminalSpawn(async () => spawned('pty-1'))

    expect(mod.isLocalTerminalSpawnEvent('pty-other')).toBe(false)
    expect(mod.isLocalTerminalSpawnEvent('pty-1')).toBe(true)
    expect(mod.isLocalTerminalSpawnEvent('pty-1')).toBe(false)
  })

  it('leaves a terminal opened elsewhere to the catalog', () => {
    expect(mod.isLocalTerminalSpawnEvent('pty-phone')).toBe(false)
  })

  it('stops claiming a late event after the window', async () => {
    vi.useFakeTimers()
    await mod.trackLocalTerminalSpawn(async () => spawned('pty-1'))

    vi.advanceTimersByTime(10_001)

    expect(mod.isLocalTerminalSpawnEvent('pty-1')).toBe(false)
  })

  it('ends the in-flight mark when the spawn fails', async () => {
    await expect(
      mod.trackLocalTerminalSpawn(() => Promise.reject(new Error('boom')))
    ).rejects.toThrow('boom')

    expect(mod.isLocalTerminalSpawnEvent('pty-1')).toBe(false)
  })
})

describe('terminalApi.spawn', () => {
  beforeEach(() => {
    vi.resetModules()
  })

  afterEach(() => {
    vi.doUnmock('./tauri-runtime')
    vi.doUnmock('./tauri-terminal-api')
  })

  // Restore, agent launch, SSH and snapshots all spawn through terminalApi
  // directly; marking only one caller left the rest to be adopted twice.
  it('marks every spawn local, whichever caller made it', async () => {
    let reply: (result: IpcResult<SpawnedTerminal>) => void = () => undefined
    const runtimeSpawn = vi.fn(
      () =>
        new Promise<IpcResult<SpawnedTerminal>>((resolve) => {
          reply = resolve
        })
    )
    vi.doMock('./tauri-runtime', () => ({ isTauriContext: () => true }))
    vi.doMock('./tauri-terminal-api', () => ({
      createTauriTerminalApi: () => ({ spawn: runtimeSpawn }),
      addRendererRef: vi.fn(),
      removeRendererRef: vi.fn(),
      setTerminalProtected: vi.fn()
    }))
    const { terminalApi } = await import('./terminal-api')
    const { isLocalTerminalSpawnEvent } = await import('./local-terminal-spawns')

    const pending = terminalApi.spawn({ projectId: 'project-a', cwd: '/a' })
    expect(runtimeSpawn).toHaveBeenCalledWith({ projectId: 'project-a', cwd: '/a' })
    expect(isLocalTerminalSpawnEvent('pty-restore')).toBe(true)

    reply(spawned('pty-restore'))
    await expect(pending).resolves.toEqual(spawned('pty-restore'))
    expect(isLocalTerminalSpawnEvent('pty-restore')).toBe(true)
  })
})
