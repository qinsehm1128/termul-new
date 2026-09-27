import { describe, expect, it } from 'vitest'
import { normalizeMcpConfig } from './mcp-api'

describe('normalizeMcpConfig oauth metadata', () => {
  it('keeps credential-free oauth metadata on canonical upstreams', () => {
    const config = normalizeMcpConfig({
      schemaVersion: 1,
      revision: 4,
      upstreams: [
        {
          id: 'remote',
          type: 'http',
          name: 'Remote',
          url: 'https://example.test/mcp',
          oauth: {
            authMode: 'oauth',
            registrationMode: 'clientMetadata',
            clientId: 'https://client.example/oauth.json',
            clientMetadataUrl: 'https://client.example/oauth.json',
            scopes: ['mcp'],
            endpoints: {
              tokenEndpoint: 'https://auth.example/token'
            },
            discoveredAt: 1700000000,
            accessToken: 'access-token-canary',
            refreshToken: 'refresh-token-canary',
            clientSecret: 'client-secret-canary'
          }
        }
      ]
    })
    expect(config.upstreams).toEqual([
      {
        id: 'remote',
        type: 'http',
        name: 'Remote',
        url: 'https://example.test/mcp',
        enabled: true,
        oauth: {
          authMode: 'oauth',
          registrationMode: 'clientMetadata',
          clientId: 'https://client.example/oauth.json',
          clientMetadataUrl: 'https://client.example/oauth.json',
          scopes: ['mcp'],
          endpoints: {
            tokenEndpoint: 'https://auth.example/token'
          },
          discoveredAt: 1700000000
        }
      }
    ])
    const encoded = JSON.stringify(config)
    expect(encoded).not.toContain('access-token-canary')
    expect(encoded).not.toContain('refresh-token-canary')
    expect(encoded).not.toContain('client-secret-canary')
  })
})
