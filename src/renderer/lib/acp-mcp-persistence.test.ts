import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/api', () => ({
  persistenceApi: { read: vi.fn(), write: vi.fn() }
}))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: vi.fn(() => true) }))
vi.mock('@/lib/web-server-api', () => ({
  webServerMcpServers: { get: vi.fn(), put: vi.fn() }
}))
vi.mock('@/lib/tauri-remote-api', () => ({
  syncMcpRegistryToProject: vi.fn(),
  loadMcpRegistryFromProject: vi.fn()
}))
vi.mock('./log-api', () => ({
  logFrontendError: vi.fn().mockResolvedValue(undefined)
}))

import { i18n } from '@/i18n'
import { persistenceApi } from '@/lib/api'
import { loadMcpRegistryFromProject, syncMcpRegistryToProject } from '@/lib/tauri-remote-api'
import { isTauriContext } from '@/lib/tauri-runtime'
import { webServerMcpServers } from '@/lib/web-server-api'
import {
  ACP_MCP_KEY,
  buildMcpServers,
  loadMcpServers,
  normalizeMcpRegistry,
  type StoredMcpServer,
  saveMcpServers,
  selectMcpServersForAgent,
  transportOf,
  validateMcpServer
} from './acp-mcp-persistence'

const registry: StoredMcpServer[] = [
  { id: 'stdio', type: 'stdio', name: 'Files', command: 'npx', enabled: true },
  { id: 'http', type: 'http', name: 'HTTP API', url: 'https://example.com/mcp', enabled: true },
  { id: 'sse', type: 'sse', name: 'Events', url: 'https://example.com/sse', enabled: true }
]

describe('MCP registry helpers', () => {
  it('validates transport-specific required fields', () => {
    expect(validateMcpServer({ type: 'stdio', name: 'fs' }).valid).toBe(false)
    expect(validateMcpServer({ type: 'stdio', name: 'fs', command: 'npx' }).valid).toBe(true)
    expect(validateMcpServer({ type: 'http', name: 'api', url: 'not a url' }).valid).toBe(false)
    expect(validateMcpServer({ type: 'sse', name: 'api', url: 'https://x.test/sse' }).valid).toBe(
      true
    )
  })

  it('localizes validation errors in Simplified Chinese', async () => {
    const previousLanguage = i18n.language
    await i18n.changeLanguage('zh-CN')
    try {
      expect(validateMcpServer({ type: 'stdio' }).errors).toEqual([
        '名称为必填项。',
        'stdio 传输必须填写命令。'
      ])
      expect(validateMcpServer({ type: 'http', name: 'api' }).errors).toEqual(['URL 为必填项。'])
      expect(validateMcpServer({ type: 'sse', name: 'api', url: 'not a url' }).errors).toEqual([
        'URL 无效。'
      ])
    } finally {
      await i18n.changeLanguage(previousLanguage)
    }
  })

  it('defaults omitted transport to stdio', () => {
    expect(transportOf({ name: 'fs', command: 'x' })).toBe('stdio')
  })

  it('normalizes legacy enabled state and warns when malformed records are skipped', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    expect(
      normalizeMcpRegistry([
        { id: 'legacy', name: 'Legacy', command: 'node' },
        { id: 'bad', name: 'Bad HTTP', type: 'http', url: 'not a url' },
        null
      ])
    ).toEqual([{ id: 'legacy', type: 'stdio', name: 'Legacy', command: 'node', enabled: true }])
    expect(warn).toHaveBeenCalledWith('[mcp] discarded 2 malformed registry entries')
    warn.mockRestore()
  })

  it('extracts upstreams from a canonical control-plane document', () => {
    expect(
      normalizeMcpRegistry({
        schemaVersion: 1,
        revision: 4,
        builtIns: [{ id: 'session-memory', enabled: true }],
        upstreams: [
          { id: 'legacy', type: 'stdio', name: 'Legacy', command: 'node', enabled: true }
        ],
        routing: { nameCollision: 'prefixServerId' }
      })
    ).toEqual([{ id: 'legacy', type: 'stdio', name: 'Legacy', command: 'node', enabled: true }])
  })

  it('selects enabled supported transports and reports unsupported servers', () => {
    expect(
      selectMcpServersForAgent(registry, { mcpCapabilities: { http: false, acp: true } })
    ).toEqual({
      servers: [{ type: 'stdio', name: 'Files', command: 'npx', args: [], env: [] }],
      skipped: [
        { id: 'http', name: 'HTTP API', transport: 'http' },
        { id: 'sse', name: 'Events', transport: 'sse' }
      ],
      pending: false
    })
    expect(selectMcpServersForAgent(registry, { mcpCapabilities: { http: true } }).servers).toEqual(
      [
        { type: 'stdio', name: 'Files', command: 'npx', args: [], env: [] },
        {
          type: 'http',
          name: 'HTTP API',
          url: 'https://example.com/mcp',
          headers: []
        }
      ]
    )
  })

  it('keeps enabled transports while capabilities are still pending', () => {
    expect(selectMcpServersForAgent(registry, null)).toEqual({
      servers: [
        { type: 'stdio', name: 'Files', command: 'npx', args: [], env: [] },
        {
          type: 'http',
          name: 'HTTP API',
          url: 'https://example.com/mcp',
          headers: []
        },
        {
          type: 'sse',
          name: 'Events',
          url: 'https://example.com/sse',
          headers: []
        }
      ],
      skipped: [],
      pending: true
    })
  })

  it('strips registry-only fields when building explicit selections', () => {
    expect(buildMcpServers(registry, ['http', 'stdio', 'sse'])).toEqual([
      { type: 'http', name: 'HTTP API', url: 'https://example.com/mcp', headers: [] },
      { type: 'stdio', name: 'Files', command: 'npx', args: [], env: [] },
      { type: 'sse', name: 'Events', url: 'https://example.com/sse', headers: [] }
    ])
  })
})

describe('registry persistence parity', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(isTauriContext).mockReturnValue(true)
    vi.mocked(loadMcpRegistryFromProject).mockResolvedValue({
      success: false,
      code: 'MCP_REGISTRY_NOT_FOUND',
      error: 'missing'
    })
    vi.mocked(syncMcpRegistryToProject).mockResolvedValue({ success: true })
  })

  it('uses the project file in Tauri and falls back to the legacy app-store key', async () => {
    vi.mocked(persistenceApi.read).mockResolvedValue({ success: true, data: registry })
    expect(await loadMcpServers()).toHaveLength(3)
    await saveMcpServers(registry)
    expect(loadMcpRegistryFromProject).toHaveBeenCalled()
    expect(persistenceApi.read).toHaveBeenCalledWith(ACP_MCP_KEY)
    expect(syncMcpRegistryToProject).toHaveBeenCalledWith(registry)
    expect(persistenceApi.write).not.toHaveBeenCalled()
  })

  it('prefers the project control-plane document over the legacy app-store key', async () => {
    vi.mocked(loadMcpRegistryFromProject).mockResolvedValue({
      success: true,
      data: {
        schemaVersion: 1,
        revision: 2,
        upstreams: [registry[0]]
      }
    })
    expect(await loadMcpServers()).toEqual([registry[0]])
    expect(persistenceApi.read).not.toHaveBeenCalled()
  })

  it('uses the shared web route outside Tauri', async () => {
    vi.mocked(isTauriContext).mockReturnValue(false)
    vi.mocked(webServerMcpServers.get).mockResolvedValue({ success: true, data: registry })
    vi.mocked(webServerMcpServers.put).mockResolvedValue({ success: true, data: undefined })
    expect(await loadMcpServers()).toHaveLength(3)
    await saveMcpServers(registry)
    expect(webServerMcpServers.get).toHaveBeenCalled()
    expect(webServerMcpServers.put).toHaveBeenCalledWith(registry)
  })

  it('returns an empty list for a missing desktop key and throws other failures', async () => {
    vi.mocked(persistenceApi.read).mockResolvedValue({
      success: false,
      code: 'KEY_NOT_FOUND',
      error: 'missing'
    })
    expect(await loadMcpServers()).toEqual([])
    vi.mocked(persistenceApi.read).mockResolvedValue({
      success: false,
      code: 'READ_ERROR',
      error: 'offline'
    })
    await expect(loadMcpServers()).rejects.toThrow('offline')
  })

  it('localizes persistence fallbacks without replacing backend errors', async () => {
    const previousLanguage = i18n.language
    await i18n.changeLanguage('zh-CN')
    try {
      vi.mocked(persistenceApi.read).mockResolvedValue({ success: false, code: 'READ_ERROR' })
      await expect(loadMcpServers()).rejects.toThrow('加载 MCP 服务器失败')

      vi.mocked(syncMcpRegistryToProject).mockResolvedValue({ success: false, code: 'WRITE_ERROR' })
      await expect(saveMcpServers([])).rejects.toThrow('保存 MCP 服务器失败')

      vi.mocked(persistenceApi.read).mockResolvedValue({
        success: false,
        code: 'READ_ERROR',
        error: 'backend offline'
      })
      await expect(loadMcpServers()).rejects.toThrow('backend offline')
    } finally {
      await i18n.changeLanguage(previousLanguage)
    }
  })
})

describe('desktop → project-file authority', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(isTauriContext).mockReturnValue(true)
    vi.mocked(syncMcpRegistryToProject).mockResolvedValue({ success: true })
  })

  it('writes only the project control-plane document on desktop', async () => {
    await saveMcpServers(registry)
    expect(syncMcpRegistryToProject).toHaveBeenCalledTimes(1)
    expect(syncMcpRegistryToProject).toHaveBeenCalledWith(registry)
    expect(persistenceApi.write).not.toHaveBeenCalled()
  })

  it('does not call syncMcpRegistryToProject on the web path', async () => {
    vi.mocked(isTauriContext).mockReturnValue(false)
    vi.mocked(webServerMcpServers.put).mockResolvedValue({ success: true, data: undefined })

    await saveMcpServers(registry)

    expect(webServerMcpServers.put).toHaveBeenCalledWith(registry)
    expect(syncMcpRegistryToProject).not.toHaveBeenCalled()
  })

  it('throws when the project-file write fails', async () => {
    vi.mocked(syncMcpRegistryToProject).mockResolvedValue({
      success: false,
      error: 'write failed',
      code: 'MCP_REGISTRY_WRITE_ERROR'
    })

    await expect(saveMcpServers(registry)).rejects.toThrow('write failed')
    expect(syncMcpRegistryToProject).toHaveBeenCalledTimes(1)
    expect(persistenceApi.write).not.toHaveBeenCalled()
  })
})
