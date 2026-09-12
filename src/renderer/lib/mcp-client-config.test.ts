import { describe, expect, it } from 'vitest'
import { buildMcpClientConfig } from './mcp-client-config'

const invocation = [
  '/Applications/Se Manager.app/Contents/MacOS/se-manager',
  '--memory-mcp-server',
  '--state-root',
  '/Users/todd/Library/Application Support/Se/state'
]

describe('buildMcpClientConfig', () => {
  it('renders the Claude-Desktop mcpServers JSON block', () => {
    const json = buildMcpClientConfig(invocation, 'json')
    const parsed = JSON.parse(json) as {
      mcpServers: Record<string, { command: string; args: string[] }>
    }
    const entry = parsed.mcpServers['se-memory']
    expect(entry.command).toBe(invocation[0])
    expect(entry.args).toEqual(invocation.slice(1))
  })

  it('renders Codex TOML with the mcp_servers table and quoted strings', () => {
    const toml = buildMcpClientConfig(invocation, 'toml')
    expect(toml).toContain('[mcp_servers.se-memory]')
    expect(toml).toContain(`command = "${invocation[0]}"`)
    expect(toml).toContain('"--memory-mcp-server", "--state-root"')
  })

  it('escapes quotes and backslashes in TOML strings', () => {
    const toml = buildMcpClientConfig(['/bin/exe', '--weird', 'a"b\\c'], 'toml')
    expect(toml).toContain('"a\\"b\\\\c"')
  })

  it('omits args entirely for a bare command', () => {
    const json = buildMcpClientConfig(['/bin/exe'], 'json')
    expect(JSON.parse(json).mcpServers['se-memory']).toEqual({ command: '/bin/exe' })
    const toml = buildMcpClientConfig(['/bin/exe'], 'toml')
    expect(toml).not.toContain('args')
  })
})
