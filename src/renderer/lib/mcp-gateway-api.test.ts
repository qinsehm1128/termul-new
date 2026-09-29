import { describe, expect, it } from 'vitest'
import { codexToml, httpServerJson, mcpServersJson } from './mcp-gateway-api'

const bridge = {
  command: 'C:\\Program Files\\Se Manager\\se-mcp.exe',
  args: ['--mode', 'entry', '--config', 'C:\\Users\\u\\.se-manager\\mcp-gateway.dev.json']
}

describe('gateway client snippets', () => {
  it('writes one se-mcp entry into mcpServers JSON', () => {
    const parsed = JSON.parse(mcpServersJson(bridge))
    expect(parsed).toEqual({ mcpServers: { 'se-mcp': bridge } })
  })

  it('writes a Codex table with TOML-escaped strings', () => {
    expect(codexToml(bridge).split('\n')).toEqual([
      '[mcp_servers.se-mcp]',
      'command = "C:\\\\Program Files\\\\Se Manager\\\\se-mcp.exe"',
      'args = ["--mode", "entry", "--config", "C:\\\\Users\\\\u\\\\.se-manager\\\\mcp-gateway.dev.json"]',
      'tool_timeout_sec = 300'
    ])
  })

  it('puts the bearer token on the HTTP entry', () => {
    const parsed = JSON.parse(
      httpServerJson({
        mode: 'grouped',
        bridge,
        bridgeAvailable: true,
        url: 'http://127.0.0.1:3290/mcp',
        token: 'se-mcp-abc',
        port: 3290,
        settingsPath: '/x'
      })
    )
    expect(parsed.mcpServers['se-mcp']).toEqual({
      type: 'http',
      url: 'http://127.0.0.1:3290/mcp',
      headers: { Authorization: 'Bearer se-mcp-abc' }
    })
  })
})
