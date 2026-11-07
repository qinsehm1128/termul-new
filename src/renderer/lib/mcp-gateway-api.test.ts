import {
  MCP_GATEWAY_CLIENT_KEYS,
  MCP_GATEWAY_SOURCES,
  type McpGatewaySource
} from '@shared/types/mcp-gateway.types'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  codexToml,
  httpServerJson,
  type McpGatewayConnection,
  mcpGatewayApi,
  mcpServersJson
} from './mcp-gateway-api'

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  desktop: true,
  log: vi.fn()
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: mocks.log }))

const TOKEN = 'TOKEN_SENTINEL_NEVER_LOG'
const bridge = (source: McpGatewaySource) => ({
  command: 'C:\\Program Files\\Se Manager\\se-mcp.exe',
  args: [
    '--source',
    source,
    '--mode',
    'direct',
    '--config',
    'C:\\Users\\u\\.se-manager\\gateway.json'
  ]
})
const connection = (source: McpGatewaySource, httpReady = true): McpGatewayConnection => ({
  source,
  httpReady,
  mode: 'direct',
  bridge: bridge(source),
  bridgeAvailable: true,
  url: `http://127.0.0.1:${source === 'internal' ? 3290 : 3390}/mcp`,
  token: TOKEN,
  port: source === 'internal' ? 3290 : 3390,
  settingsPath: '/x'
})

beforeEach(() => {
  vi.clearAllMocks()
  mocks.desktop = true
  mocks.invoke.mockResolvedValue({ success: true, data: {} })
})

describe.each(MCP_GATEWAY_SOURCES)('%s client snippets', (source) => {
  const key = MCP_GATEWAY_CLIENT_KEYS[source]
  it('writes exactly the selected JSON key and explicit source, without HTTP secrets', () => {
    const commandWithExtraFields = { ...bridge(source), token: TOKEN, url: 'SECRET_URL' }
    const snippet = mcpServersJson(source, commandWithExtraFields)
    expect(JSON.parse(snippet)).toEqual({ mcpServers: { [key]: bridge(source) } })
    expect(snippet).not.toContain(TOKEN)
    expect(snippet).not.toContain('SECRET_URL')
  })
  it('writes exactly the selected Codex table and escaped strings', () => {
    const snippet = codexToml(source, bridge(source))
    expect(snippet.split('\n')).toEqual([
      `[mcp_servers.${key}]`,
      'command = "C:\\\\Program Files\\\\Se Manager\\\\se-mcp.exe"',
      `args = ["--source", "${source}", "--mode", "direct", "--config", "C:\\\\Users\\\\u\\\\.se-manager\\\\gateway.json"]`,
      'tool_timeout_sec = 300'
    ])
    expect(snippet).not.toContain(TOKEN)
  })
  it('selects the HTTP key from connection.source only after verified readiness', () => {
    const value = connection(source)
    expect(JSON.parse(httpServerJson(value))).toEqual({
      mcpServers: {
        [key]: { type: 'http', url: value.url, headers: { Authorization: `Bearer ${TOKEN}` } }
      }
    })
    expect(() => httpServerJson(connection(source, false))).toThrow('MCP_HTTP_NOT_READY')
    expect(() =>
      httpServerJson({ ...value, httpReady: undefined } as unknown as McpGatewayConnection)
    ).toThrow('MCP_HTTP_NOT_READY')
  })
})

describe.each(MCP_GATEWAY_SOURCES)('%s desktop facade', (source) => {
  it('passes source on every source-scoped IPC command', async () => {
    await mcpGatewayApi.view(source)
    await mcpGatewayApi.start(source)
    await mcpGatewayApi.restart(source)
    await mcpGatewayApi.setPort(source, 5555)
    await mcpGatewayApi.connection(source, 'entry')
    await mcpGatewayApi.clients(source, 'direct')
    await mcpGatewayApi.syncClient(source, 'codex', 'grouped')
    await mcpGatewayApi.unsyncClient(source, 'codex')
    await mcpGatewayApi.servers(source)
    expect(mocks.invoke.mock.calls).toEqual([
      ['mcp_get_runtime_status', { source }],
      ['mcp_service_start', { source }],
      ['mcp_service_restart', { source }],
      ['mcp_service_set_port', { source, port: 5555 }],
      ['mcp_connection_info', { source, mode: 'entry' }],
      ['mcp_clients_detect', { source, mode: 'direct' }],
      ['mcp_client_sync', { source, id: 'codex', mode: 'grouped' }],
      ['mcp_client_unsync', { source, id: 'codex' }],
      ['mcp_service_servers', { source }]
    ])
  })
  it('returns desktop-only without any local invoke in browser/shared-live/standalone', async () => {
    mocks.desktop = false
    const responses = await Promise.all([
      mcpGatewayApi.view(source),
      mcpGatewayApi.start(source),
      mcpGatewayApi.restart(source),
      mcpGatewayApi.setPort(source, 5555),
      mcpGatewayApi.connection(source, 'direct'),
      mcpGatewayApi.clients(source, 'grouped'),
      mcpGatewayApi.syncClient(source, 'codex', 'entry'),
      mcpGatewayApi.unsyncClient(source, 'codex'),
      mcpGatewayApi.servers(source)
    ])
    for (const response of responses)
      expect(response).toMatchObject({ success: false, code: 'DESKTOP_ONLY' })
    expect(mocks.invoke).not.toHaveBeenCalled()
    expect(mocks.log).not.toHaveBeenCalled()
  })
  it('logs stable metadata, never raw errors, tokens or connection payloads', async () => {
    mocks.invoke.mockRejectedValueOnce(new Error(`RAW_ERROR ${TOKEN} Bearer`))
    expect(await mcpGatewayApi.connection(source, 'direct')).toEqual({
      success: false,
      code: 'INVOKE_ERROR',
      error: 'MCP gateway request failed'
    })
    mocks.invoke.mockResolvedValueOnce({ success: false, error: `RAW_ERROR ${TOKEN}` })
    await mcpGatewayApi.start(source)
    const logged = JSON.stringify(mocks.log.mock.calls)
    expect(logged).toContain(`source=${source}`)
    expect(logged).toContain('MCP_GATEWAY_REQUEST_FAILED')
    expect(logged).not.toContain(TOKEN)
    expect(logged).not.toContain('RAW_ERROR')
    expect(logged).not.toContain('Bearer')
  })
})
