import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn()
}))
vi.mock('@/lib/tauri-runtime', () => ({
  isTauriContext: vi.fn(() => false)
}))

import { invoke } from '@tauri-apps/api/core'
import { isTauriContext } from '@/lib/tauri-runtime'
import { beginMcpOAuth, cancelMcpOAuth, getMcpOAuthStatus, normalizeMcpConfig } from './mcp-api'

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
              tokenEndpoint: 'https://auth.example/token',
              resource: 'https://example.test/mcp'
            },
            redirectUri: 'http://127.0.0.1:43111/oauth/callback',
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
            tokenEndpoint: 'https://auth.example/token',
            resource: 'https://example.test/mcp'
          },
          redirectUri: 'http://127.0.0.1:43111/oauth/callback',
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

describe('OAuth desktop boundary', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(isTauriContext).mockReturnValue(false)
  })

  it('reports unsupported OAuth on the web runtime without invoking the host', async () => {
    const begin = await beginMcpOAuth('remote', 'Bearer resource_metadata="https://example.test"')
    const status = await getMcpOAuthStatus('remote')
    const cancel = await cancelMcpOAuth('remote')
    for (const result of [begin, status, cancel]) {
      expect(result.success).toBe(false)
      if (!result.success) {
        expect(result.code).toBe('OAUTH_UNSUPPORTED_RUNTIME')
        expect(result.error).toContain('desktop app')
        expect(result.error).not.toContain('access_token')
      }
    }
    expect(invoke).not.toHaveBeenCalled()
  })

  it('invokes the desktop OAuth command only in Tauri', async () => {
    vi.mocked(isTauriContext).mockReturnValue(true)
    vi.mocked(invoke).mockResolvedValue({
      serverId: 'remote',
      authorizationUrl: 'http://127.0.0.1:9/authorize',
      expiresAt: 10,
      state: 'pending'
    })
    const result = await beginMcpOAuth('remote')
    expect(result.success).toBe(true)
    expect(invoke).toHaveBeenCalledWith('begin_mcp_oauth', {
      id: 'remote',
      challenge: undefined
    })
  })
})
