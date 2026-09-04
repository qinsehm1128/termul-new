import { renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useTerminalResourceLifecycle } from '@/hooks/use-terminal-resource-lifecycle'
import { terminalApi } from '@/lib/terminal-api'
import { useSessionWorkspaceSyncStore } from '@/stores/session-workspace-sync-store'
import { useTerminalStore } from '@/stores/terminal-store'

const conversationId = '018f7a1c-1b4d-7c8a-9f01-0123456789ab'

let exitHandler: ((ptyId: string, exitCode: number | null) => void) | undefined

vi.mock('@/lib/terminal-api', () => ({
  terminalApi: {
    onExit: vi.fn((handler: (ptyId: string, exitCode: number | null) => void) => {
      exitHandler = handler
      return () => {
        exitHandler = undefined
      }
    })
  }
}))

vi.mock('@/hooks/use-session-workspace-sync', () => ({
  performSessionWorkspaceWrite: vi.fn(async () => undefined)
}))

function seedTerminal(): void {
  useTerminalStore.setState({
    terminals: [
      {
        id: 'record-1',
        name: 'shell',
        ptyId: 'pty-1',
        conversationId,
        healthStatus: 'running',
        viewState: 'visible'
      }
    ] as never,
    ptyIdIndex: new Map([['pty-1', 'record-1']])
  } as never)
}

function healthStatus(): string | undefined {
  return useTerminalStore.getState().terminals.find((t) => t.id === 'record-1')?.healthStatus
}

describe('useTerminalResourceLifecycle exit mapping', () => {
  beforeEach(() => {
    exitHandler = undefined
    useSessionWorkspaceSyncStore.setState({ activeConversationId: conversationId } as never)
    seedTerminal()
    renderHook(() => useTerminalResourceLifecycle())
    expect(terminalApi.onExit).toHaveBeenCalled()
  })

  // The defect: every exit was mapped to `crashed`, so an ordinary `exit 0`
  // raised a pane-exception surface over a terminal nobody had lost.
  it('maps a clean exit to exited, not crashed', () => {
    exitHandler?.('pty-1', 0)
    expect(healthStatus()).toBe('exited')
  })

  it('still maps a non-zero exit to crashed', () => {
    exitHandler?.('pty-1', 1)
    expect(healthStatus()).toBe('crashed')
  })

  it('treats a missing exit code as a crash, not a clean exit', () => {
    exitHandler?.('pty-1', null)
    expect(healthStatus()).toBe('crashed')
  })

  it('leaves an already-disconnected terminal disconnected', () => {
    useTerminalStore.getState().setTerminalHealthStatus('record-1', 'disconnected')
    exitHandler?.('pty-1', 0)
    expect(healthStatus()).toBe('disconnected')
  })
})
