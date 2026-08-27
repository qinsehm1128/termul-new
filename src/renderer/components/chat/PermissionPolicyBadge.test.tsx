import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AcpSession } from '@/stores/acp-store'
import { PermissionPolicyBadge } from './PermissionPolicyBadge'

const { stateRef, saveAgentConfig } = vi.hoisted(() => ({
  saveAgentConfig: vi.fn(),
  stateRef: {
    current: {
      agentConfigs: [
        {
          id: 'cfg-1',
          configId: 'cfg-1',
          name: 'Codex',
          command: 'codex-acp',
          args: [],
          env: {},
          permissionPolicy: 'ask'
        }
      ] as Array<{
        id: string
        configId: string
        name: string
        command: string
        args: string[]
        env: Record<string, string>
        permissionPolicy: 'ask' | 'allow_all'
      }>,
      configToLiveAgent: { 'cfg-1\0/work': 'agent-1' },
      saveAgentConfig: vi.fn()
    }
  }
}))

vi.mock('sonner', () => ({
  toast: { error: vi.fn() }
}))

vi.mock('@/stores/acp-store', () => ({
  useAcpStore: (selector: (state: typeof stateRef.current) => unknown) => selector(stateRef.current)
}))

const SESSION = {
  id: 'session-1',
  agentId: 'agent-1',
  cwd: '/work',
  projectId: 'project-1',
  status: 'ready',
  title: null,
  activeTurn: false,
  openTurnId: null,
  modes: null,
  configOptions: [],
  lastError: null,
  createdAt: 1
} as AcpSession

describe('PermissionPolicyBadge', () => {
  beforeEach(() => {
    saveAgentConfig.mockReset()
    stateRef.current.saveAgentConfig = saveAgentConfig
    stateRef.current.agentConfigs[0] = {
      ...stateRef.current.agentConfigs[0],
      permissionPolicy: 'ask'
    }
  })

  it('shows the current policy in the chat composer', () => {
    render(<PermissionPolicyBadge session={SESSION} />)
    expect(screen.getByRole('button', { name: /tool permission policy: ask/i })).toBeInTheDocument()
  })

  it('requires confirmation before enabling full access from chat', async () => {
    saveAgentConfig.mockResolvedValue(undefined)
    render(<PermissionPolicyBadge session={SESSION} />)

    fireEvent.click(screen.getByRole('button', { name: /tool permission policy: ask/i }))
    fireEvent.click(screen.getByRole('switch', { name: 'Allow all tool requests' }))
    expect(saveAgentConfig).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Enable full access' }))
    await waitFor(() =>
      expect(saveAgentConfig).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'cfg-1', permissionPolicy: 'allow_all' })
      )
    )
  })

  it('returns to ask mode immediately when full access is disabled', async () => {
    stateRef.current.agentConfigs[0] = {
      ...stateRef.current.agentConfigs[0],
      permissionPolicy: 'allow_all'
    }
    saveAgentConfig.mockResolvedValue(undefined)
    render(<PermissionPolicyBadge session={SESSION} />)

    fireEvent.click(screen.getByRole('button', { name: /tool permission policy: full access/i }))
    fireEvent.click(screen.getByRole('switch', { name: 'Allow all tool requests' }))

    await waitFor(() =>
      expect(saveAgentConfig).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'cfg-1', permissionPolicy: 'ask' })
      )
    )
  })
})
