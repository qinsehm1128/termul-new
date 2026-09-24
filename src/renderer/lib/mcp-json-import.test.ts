import { describe, expect, it } from 'vitest'
import { parseMcpJsonImport } from './mcp-json-import'
import { buildMcpJsonExport, prepareMcpJsonImport } from './mcp-json-transfer'

describe('parseMcpJsonImport', () => {
  it('parses a Claude Desktop wrapper and normalizes the env map to name/value pairs', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          dokploy: {
            command: 'npx',
            args: ['-y', '@dokploy/mcp'],
            env: { DOKPLOY_URL: 'https://dokploy.test', DOKPLOY_API_KEY: 'secret' }
          }
        }
      })
    )
    expect(errors).toEqual([])
    expect(servers).toEqual([
      {
        type: 'stdio',
        name: 'dokploy',
        command: 'npx',
        args: ['-y', '@dokploy/mcp'],
        env: [
          { name: 'DOKPLOY_URL', value: 'https://dokploy.test' },
          { name: 'DOKPLOY_API_KEY', value: 'secret' }
        ]
      }
    ])
  })

  it('parses a bare single-server object using its name field', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        command: 'npx',
        args: ['-y', '@dokploy/mcp'],
        env: { DOKPLOY_URL: 'https://dokploy.test' },
        name: 'dokploy'
      })
    )
    expect(errors).toEqual([])
    expect(servers).toEqual([
      {
        type: 'stdio',
        name: 'dokploy',
        command: 'npx',
        args: ['-y', '@dokploy/mcp'],
        env: [{ name: 'DOKPLOY_URL', value: 'https://dokploy.test' }]
      }
    ])
  })

  it('drops unknown fields such as directTools and alwaysAllow', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          x: {
            command: 'node',
            args: ['server.js'],
            directTools: true,
            alwaysAllow: ['read']
          }
        }
      })
    )
    expect(errors).toEqual([])
    expect(servers).toHaveLength(1)
    expect(servers[0]).toEqual({
      type: 'stdio',
      name: 'x',
      command: 'node',
      args: ['server.js']
    })
    expect(JSON.stringify(servers[0])).not.toContain('directTools')
    expect(JSON.stringify(servers[0])).not.toContain('alwaysAllow')
  })

  it('passes an env that is already a name/value array through unchanged', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          a: { command: 'node', env: [{ name: 'K', value: 'v' }] }
        }
      })
    )
    expect(errors).toEqual([])
    expect(servers[0].env).toEqual([{ name: 'K', value: 'v' }])
  })

  it('parses multiple wrapper servers, each named by its key', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          a: { command: 'node', args: ['a.js'] },
          b: { command: 'python', args: ['b.py'] }
        }
      })
    )
    expect(errors).toEqual([])
    expect(servers.map((server) => server.name)).toEqual(['a', 'b'])
    expect(servers[0].command).toBe('node')
    expect(servers[1].command).toBe('python')
  })

  it('infers http when only a url is present', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          remote: { url: 'https://mcp.test/mcp' }
        }
      })
    )
    expect(errors).toEqual([])
    expect(servers[0]).toEqual({ type: 'http', name: 'remote', url: 'https://mcp.test/mcp' })
  })

  it('honors an explicit sse type', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          legacy: { type: 'sse', url: 'https://mcp.test/sse' }
        }
      })
    )
    expect(errors).toEqual([])
    expect(servers[0]).toEqual({ type: 'sse', name: 'legacy', url: 'https://mcp.test/sse' })
  })

  it('rejects a server with an env in an unknown shape but keeps the rest', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          bad: { command: 'node', env: 'KEY=value' },
          good: { command: 'node' }
        }
      })
    )
    expect(servers.map((server) => server.name)).toEqual(['good'])
    expect(errors).toEqual(['bad: env must be an object map or name/value pairs'])
  })

  it('returns an Invalid JSON error for truncated input', () => {
    const { servers, errors } = parseMcpJsonImport('{"mcpServers":{')
    expect(servers).toEqual([])
    expect(errors).toHaveLength(1)
    expect(errors[0]).toMatch(/^Invalid JSON: /)
  })

  it('rejects a server missing a required field with a per-server error', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          x: { args: ['-y'] }
        }
      })
    )
    expect(servers).toEqual([])
    expect(errors).toEqual(['x: Command is required for stdio.'])
  })

  it('rejects a top-level mcpServers that is not an object', () => {
    const { servers, errors } = parseMcpJsonImport('{"mcpServers": []}')
    expect(servers).toEqual([])
    expect(errors).toEqual(['Invalid JSON: "mcpServers" must be an object'])
  })

  it('accepts an empty top-level array as an empty import', () => {
    expect(parseMcpJsonImport('[]')).toEqual({ servers: [], errors: [] })
  })

  it('prefers the rich servers inventory and preserves autoStart plus inline credentials', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        version: '1.0.0',
        servers: [
          {
            name: 'remote',
            command: '',
            args: [],
            env: { Authorization: 'Bearer inline-secret' },
            serverType: 'remote-streamable',
            remoteUrl: 'https://mcp.test/mcp',
            autoStart: false
          }
        ],
        mcpServers: {
          remote: { command: 'mcp-proxy', args: ['https://wrong.test/mcp'] }
        }
      })
    )

    expect(errors).toEqual([])
    expect(servers).toEqual([
      {
        type: 'http',
        name: 'remote',
        url: 'https://mcp.test/mcp',
        headers: [{ name: 'Authorization', value: 'Bearer inline-secret' }],
        enabled: false
      }
    ])
  })

  it('accepts Qin/tauri-mcp-router mcpServers entries and maps remoteUrl', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          remote: {
            remoteUrl: 'https://mcp.test/stream',
            serverType: 'remote-streamable',
            bearerToken: 'token-remote',
            requestOptions: { headers: { 'X-Tenant': 'tenant-a' } }
          },
          local: {
            server_type: 'local',
            command: 'node',
            args: ['server.js'],
            env: { TOKEN: 'secret' }
          }
        }
      })
    )
    expect(errors).toEqual([])
    expect(servers).toEqual([
      {
        type: 'http',
        name: 'remote',
        url: 'https://mcp.test/stream',
        headers: [
          { name: 'X-Tenant', value: 'tenant-a' },
          { name: 'Authorization', value: 'Bearer token-remote' }
        ]
      },
      {
        type: 'stdio',
        name: 'local',
        command: 'node',
        args: ['server.js'],
        env: [{ name: 'TOKEN', value: 'secret' }]
      }
    ])
  })

  it('accepts remote aliases, bearer tokens, and request/header maps', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        servers: [
          {
            name: 'remote-a',
            remote_url: 'https://a.test/mcp',
            server_type: 'http',
            bearer_token: 'token-a',
            headers: { 'X-Header': 'header-a' }
          },
          {
            name: 'remote-b',
            serverUrl: 'https://b.test/mcp',
            request_options: { headers: { Authorization: 'Bearer token-b', 'X-Org': 'org-b' } }
          },
          {
            name: 'remote-c',
            url: 'https://c.test/mcp',
            http_headers: { 'X-Workspace': 'workspace-c' }
          }
        ]
      })
    )
    expect(errors).toEqual([])
    expect(servers).toEqual([
      {
        type: 'http',
        name: 'remote-a',
        url: 'https://a.test/mcp',
        headers: [
          { name: 'X-Header', value: 'header-a' },
          { name: 'Authorization', value: 'Bearer token-a' }
        ]
      },
      {
        type: 'http',
        name: 'remote-b',
        url: 'https://b.test/mcp',
        headers: [
          { name: 'Authorization', value: 'Bearer token-b' },
          { name: 'X-Org', value: 'org-b' }
        ]
      },
      {
        type: 'http',
        name: 'remote-c',
        url: 'https://c.test/mcp',
        headers: [{ name: 'X-Workspace', value: 'workspace-c' }]
      }
    ])
  })

  it('accepts canonical upstreams while ignoring built-ins and routing metadata', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        schemaVersion: 1,
        builtIns: [{ id: 'session-memory', enabled: false }],
        upstreams: [
          { id: 'one', name: 'One', type: 'stdio', command: 'node', enabled: false },
          {
            id: 'two',
            name: 'Two',
            type: 'http',
            url: 'https://two.test/mcp',
            headers: [{ name: 'Authorization', value: 'Bearer secret' }]
          }
        ],
        routing: { nameCollision: 'prefixServerId' }
      })
    )
    expect(errors).toEqual([])
    expect(servers).toEqual([
      { type: 'stdio', name: 'One', command: 'node', enabled: false },
      {
        type: 'http',
        name: 'Two',
        url: 'https://two.test/mcp',
        headers: [{ name: 'Authorization', value: 'Bearer secret' }]
      }
    ])
  })

  it('accepts mcp-proxy command URLs as remote HTTP entries', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        servers: {
          proxy: { command: 'mcp-proxy', args: ['--url', 'https://proxy.test/mcp'] }
        }
      })
    )
    expect(errors).toEqual([])
    expect(servers).toEqual([{ type: 'http', name: 'proxy', url: 'https://proxy.test/mcp' }])
  })

  it('skips duplicate names and malformed entries without importing secrets into errors', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify([
        { name: 'same', command: 'node', env: { TOKEN: 'secret-value' } },
        { name: 'SAME', command: 'bun' },
        { name: 'bad', command: 'node', args: [1] },
        { name: 'good', command: 'node' }
      ])
    )
    expect(servers.map((server) => server.name)).toEqual(['same', 'good'])
    expect(errors).toEqual(['SAME: duplicate server skipped', 'bad: invalid server configuration'])
    expect(errors.join(' ')).not.toContain('secret-value')
  })

  it('drops only explicitly referenced secret pairs', () => {
    const { servers, errors } = parseMcpJsonImport(
      JSON.stringify({
        mcpServers: {
          remote: {
            url: 'https://mcp.test/mcp',
            headers: [
              { name: 'Authorization', ref: 'mcp/remote/authorization' },
              { name: 'X-Visible', value: 'visible' }
            ]
          }
        }
      })
    )
    expect(errors).toEqual([])
    expect(servers[0]?.headers).toEqual([{ name: 'X-Visible', value: 'visible' }])
  })

  it('prepares a fresh-id append batch and skips names already in the registry', () => {
    const result = prepareMcpJsonImport(
      JSON.stringify({
        servers: {
          existing: { command: 'node' },
          fresh: { command: 'bun' }
        }
      }),
      [{ name: 'existing' }],
      (() => {
        let index = 0
        return () => `fresh-${++index}`
      })()
    )
    expect(result.errors).toEqual([])
    expect(result.skipped).toBe(1)
    expect(result.servers).toEqual([
      { id: 'fresh-1', type: 'stdio', name: 'fresh', command: 'bun', enabled: true }
    ])
  })

  it('exports canonical config with a standard mcpServers compatibility map', () => {
    const exported = buildMcpJsonExport({
      schemaVersion: 1,
      revision: 4,
      builtIns: [{ id: 'session-memory', enabled: true }],
      upstreams: [
        {
          id: 'local-id',
          type: 'stdio',
          name: 'Local',
          command: 'node',
          args: ['server.js'],
          env: [{ name: 'TOKEN', value: 'secret' }],
          enabled: false
        },
        {
          id: 'remote-id',
          type: 'http',
          name: 'Remote',
          url: 'https://remote.test/mcp',
          headers: [{ name: 'X-Org', value: 'org' }],
          enabled: true
        }
      ],
      routing: { nameCollision: 'prefixServerId' }
    })
    expect(exported.schemaVersion).toBe(1)
    expect(exported.upstreams[0]).toMatchObject({ id: 'local-id', enabled: false })
    expect(exported.mcpServers).toEqual({
      Local: { command: 'node', args: ['server.js'], env: { TOKEN: 'secret' } },
      Remote: { url: 'https://remote.test/mcp', headers: { 'X-Org': 'org' } }
    })
  })
})
