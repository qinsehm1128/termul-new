import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { StoredAgentConfig } from '@/lib/acp-agents-persistence'
import type { SupportedAcpAgentEntry } from '@/lib/agents/supported-acp-agents'
import { buildSupportedAcpAgents } from '@/lib/agents/supported-acp-agents'
import { AcpAgentsSettings } from './AcpAgentsSettings'

const { stateRef, resolvedRef } = vi.hoisted(() => ({
  stateRef: {
    current: {
      agentConfigs: [] as StoredAgentConfig[],
      warmingConfigs: {},
      configToLiveAgent: {},
      agentStatus: {},
      saveAgentConfig: vi.fn(),
      deleteAgentConfig: vi.fn()
    }
  },
  // When non-null, `useResolvedSupportedAcpAgents` returns these entries
  // verbatim (for tests that need a custom-agent row the sync
  // `buildSupportedAcpAgents` helper doesn't synthesize). Otherwise it falls
  // back to the real sync derivation over the bundled registry.
  resolvedRef: { current: null as SupportedAcpAgentEntry[] | null }
}))

vi.mock('@tauri-apps/plugin-os', () => ({
  platform: vi.fn(() => 'windows'),
  arch: vi.fn(() => 'x86_64')
}))

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), error: vi.fn() }
}))

vi.mock('@/stores/acp-store', () => {
  const useAcpStore = (sel?: (s: typeof stateRef.current) => unknown) =>
    sel ? sel(stateRef.current) : stateRef.current
  const useConfigWarmState = () => ({
    connected: false,
    warming: false,
    warmingSession: false,
    sessionReady: false
  })
  return { useAcpStore, useConfigWarmState }
})

vi.mock('@/hooks/use-acp-runtime-probe', () => ({
  useAcpRuntimeProbe: () => ({ npx: true, uvx: true })
}))

vi.mock('@/hooks/use-resolved-supported-acp-agents', async () => {
  const actual = await vi.importActual<typeof import('@/lib/agents/supported-acp-agents')>(
    '@/lib/agents/supported-acp-agents'
  )
  return {
    useResolvedSupportedAcpAgents: (configs: readonly StoredAgentConfig[]) =>
      resolvedRef.current ?? actual.buildSupportedAcpAgents(configs, 'windows-x86_64')
  }
})

describe('AcpAgentsSettings', () => {
  beforeEach(() => {
    stateRef.current.agentConfigs = []
    stateRef.current.deleteAgentConfig = vi.fn()
    stateRef.current.saveAgentConfig = vi.fn()
    resolvedRef.current = null
  })

  it('shows supported ACP agent status and permission policy controls', () => {
    const entries = buildSupportedAcpAgents([], 'windows-x86_64')
    render(<AcpAgentsSettings />)

    expect(screen.getByText('Claude Agent')).toBeInTheDocument()
    expect(screen.getByText('Codex')).toBeInTheDocument()
    expect(screen.getByText('Gemini CLI')).toBeInTheDocument()
    expect(screen.getByText('Cursor')).toBeInTheDocument()
    expect(screen.getByText('OpenCode')).toBeInTheDocument()
    expect(screen.getByText('pi ACP')).toBeInTheDocument()
    expect(screen.getAllByRole('switch')).toHaveLength(
      entries.filter((entry) => entry.config !== null).length
    )
    expect(screen.getAllByText('Install from Agent Chat')).toHaveLength(
      entries.filter((entry) => entry.status === 'install-required').length
    )
  })

  it('deletes a custom agent by its stored record id, not its configId (CodeRabbit)', () => {
    // A custom agent pasted with an exported configId keeps that configId but
    // gets a fresh stored `id`. `clearPath` must delete by the stored `id` or
    // `deleteAgentConfig` (which filters by `c.id`) would miss it.
    const stored: StoredAgentConfig = {
      id: 'custom-xyz',
      configId: 'custom-honored',
      name: 'Internal Helper',
      command: 'node',
      args: ['/path/to/agent.js'],
      env: {},
      allowTerminal: false,
      permissionPolicy: 'ask'
    }
    resolvedRef.current = [
      {
        id: stored.id,
        configId: stored.configId,
        agent: { id: stored.id, name: stored.name, version: '', description: '', distribution: {} },
        config: stored,
        status: 'ready',
        install: null,
        manualInstall: null,
        runtimeLauncher: null,
        unavailableReason: null
      }
    ]
    render(<AcpAgentsSettings />)

    const clearBtn = screen.getByRole('button', { name: /clear saved path/i })
    fireEvent.click(clearBtn)

    expect(stateRef.current.deleteAgentConfig).toHaveBeenCalledTimes(1)
    expect(stateRef.current.deleteAgentConfig).toHaveBeenCalledWith('custom-xyz')
  })

  it('requires confirmation before enabling the allow-all permission policy', async () => {
    const stored: StoredAgentConfig = {
      id: 'custom-policy',
      configId: 'custom-policy',
      name: 'Trusted Agent',
      command: 'trusted-agent',
      args: [],
      env: {},
      allowTerminal: false,
      permissionPolicy: 'ask'
    }
    resolvedRef.current = [
      {
        id: stored.id,
        configId: stored.configId!,
        agent: { id: stored.id, name: stored.name, version: '', description: '', distribution: {} },
        config: stored,
        status: 'ready',
        install: null,
        manualInstall: null,
        runtimeLauncher: null,
        unavailableReason: null
      }
    ]
    stateRef.current.saveAgentConfig = vi.fn().mockResolvedValue(undefined)
    render(<AcpAgentsSettings />)

    fireEvent.click(screen.getByRole('switch', { name: /trusted agent/i }))
    expect(stateRef.current.saveAgentConfig).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Enable full access' }))

    await waitFor(() =>
      expect(stateRef.current.saveAgentConfig).toHaveBeenCalledWith({
        ...stored,
        permissionPolicy: 'allow_all'
      })
    )
  })
})
