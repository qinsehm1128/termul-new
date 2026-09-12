import { describe, expect, it } from 'vitest'
import { buildMcpClientConfig, memoryMcpServerKey } from './mcp-client-config'

const invocation = [
  '/Applications/Se Manager.app/Contents/MacOS/se-manager',
  '--memory-mcp',
  '--project',
  '/Users/todd/My Project',
  '--state-root',
  '/Users/todd/Library/Application Support/Se/state'
]

describe('memoryMcpServerKey', () => {
  it('slugifies the project folder name', () => {
    expect(memoryMcpServerKey('My Project')).toBe('se-memory-my-project')
    expect(memoryMcpServerKey('  -- weird__name!! ')).toBe('se-memory-weird-name')
  })

  it('falls back to a bare key when the name slugifies away', () => {
    expect(memoryMcpServerKey('!!!')).toBe('se-memory')
  })
})

describe('buildMcpClientConfig', () => {
  it('renders the Claude-Desktop mcpServers JSON block', () => {
    const json = buildMcpClientConfig(invocation, 'json', 'My Project')
    const parsed = JSON.parse(json) as {
      mcpServers: Record<string, { command: string; args: string[] }>
    }
    const entry = parsed.mcpServers['se-memory-my-project']
    expect(entry.command).toBe(invocation[0])
    expect(entry.args).toEqual(invocation.slice(1))
  })

  it('renders Codex TOML with the mcp_servers table and quoted strings', () => {
    const toml = buildMcpClientConfig(invocation, 'toml', 'My Project')
    expect(toml).toContain('[mcp_servers.se-memory-my-project]')
    expect(toml).toContain(`command = "${invocation[0]}"`)
    expect(toml).toContain(`"--project", "/Users/todd/My Project"`)
  })

  it('escapes quotes and backslashes in TOML strings', () => {
    const toml = buildMcpClientConfig(['/bin/exe', '--weird', 'a"b\\c'], 'toml', 'proj')
    expect(toml).toContain('"a\\"b\\\\c"')
  })

  it('omits args entirely for a bare command', () => {
    const json = buildMcpClientConfig(['/bin/exe'], 'json', 'p')
    expect(JSON.parse(json).mcpServers['se-memory-p']).toEqual({ command: '/bin/exe' })
    const toml = buildMcpClientConfig(['/bin/exe'], 'toml', 'p')
    expect(toml).not.toContain('args')
  })
})
