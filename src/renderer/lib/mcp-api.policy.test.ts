import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: vi.fn(() => false) }))
vi.mock('@/lib/web-server-api', () => ({
  webServerMcpControl: { getConfig: vi.fn(), putConfig: vi.fn() }
}))

import { invoke } from '@tauri-apps/api/core'
import { isTauriContext } from '@/lib/tauri-runtime'
import { webServerMcpControl } from '@/lib/web-server-api'
import { getMcpConfig, normalizeMcpConfig, putMcpConfig } from './mcp-api'

function normalizePolicy(policy: unknown) {
  return normalizeMcpConfig({
    builtIns: [{ id: 'project-scope', enabled: false, policy }],
    upstreams: []
  }).builtIns.find((item) => item.id === 'project-scope')
}

describe('MCP built-in policy normalization', () => {
  it('preserves legacy disabled and allow/deny restrictions through repeated normalization', () => {
    const input = {
      builtIns: [
        {
          id: 'project-scope',
          enabled: false,
          policy: { allowTools: ['memory_projects'], denyTools: ['project_boundary'] }
        }
      ],
      upstreams: []
    }
    const once = normalizeMcpConfig(input)
    const twice = normalizeMcpConfig(once)
    expect(once.builtIns.find((item) => item.id === 'project-scope')).toEqual(input.builtIns[0])
    expect(twice).toEqual(once)
  })

  it('keeps an explicit empty allow list as deny-all', () => {
    expect(normalizePolicy({ allowTools: [] })?.policy).toEqual({ allowTools: [] })
  })

  it('does not add restrictions when a policy was not configured', () => {
    expect(normalizePolicy(undefined)?.policy).toBeUndefined()
    expect(normalizePolicy({})?.policy).toEqual({})
  })

  it.each(
    [null, false, [], 'all', { allowTools: 'memory_projects' }, { denyTools: [7] }].map(
      (policy) => ({ policy })
    )
  )('fails closed on malformed policy $policy', ({ policy }) => {
    expect(normalizePolicy(policy)?.policy).toEqual({ allowTools: [] })
  })

  it('copies lists without mutating the incoming configuration', () => {
    const allowTools = ['memory_projects']
    const denyTools = ['project_boundary']
    const result = normalizePolicy({ allowTools, denyTools })
    result?.policy?.allowTools?.push('another_tool')
    expect(allowTools).toEqual(['memory_projects'])
    expect(denyTools).toEqual(['project_boundary'])
  })

  it.each([
    true,
    false
  ])('preserves policy on the real load/save facade (desktop=%s)', async (desktop) => {
    vi.clearAllMocks()
    vi.mocked(isTauriContext).mockReturnValue(desktop)
    const config = normalizeMcpConfig({
      revision: 7,
      builtIns: [{ id: 'session-memory', enabled: true, policy: { allowTools: [] } }],
      upstreams: []
    })
    const response = { success: true as const, data: config }
    vi.mocked(invoke).mockResolvedValue(response)
    vi.mocked(webServerMcpControl.getConfig).mockResolvedValue(response)
    vi.mocked(webServerMcpControl.putConfig).mockResolvedValue(response)

    const loaded = await getMcpConfig()
    expect(loaded.success).toBe(true)
    if (!loaded.success) throw new Error('Expected successful configuration read')
    const saved = await putMcpConfig(loaded.data)
    expect(saved).toEqual(response)
    if (desktop) {
      expect(invoke).toHaveBeenCalledWith('mcp_put_config', { config })
      expect(webServerMcpControl.putConfig).not.toHaveBeenCalled()
    } else {
      expect(webServerMcpControl.putConfig).toHaveBeenCalledWith(config)
      expect(invoke).not.toHaveBeenCalled()
    }
  })
})
