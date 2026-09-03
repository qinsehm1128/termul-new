import { brandCanonical } from '@shared/brand'
import { describe, expect, it } from 'vitest'
import {
  appDataCandidates,
  DEFAULT_LOCAL_ORIGIN,
  e2ePrompt,
  HISTORY_TAIL_WINDOW,
  historyAfterSeq,
  historyContainsMarker,
  listenPortsFromLsof,
  normalizeWsReply,
  pairingTokenFromSecrets,
  parseRemoteAccessIntent,
  pickBrowsableFile,
  pickLiveAgentId,
  pickPermissionOption,
  redactOrigin,
  remoteAccessIntentPaths,
  secretsPaths,
  subscribePayload,
  tokenFromAccessUrl,
  wsUrl
} from './mobile-host-probe'

describe('mobile-host-probe helpers', () => {
  it('reads the pairing token from a credentialed access URL and drops the fragment', () => {
    const parsed = tokenFromAccessUrl('http://127.0.0.1:18787/#access_token=secret-token')
    expect(parsed).toEqual({ origin: 'http://127.0.0.1:18787', token: 'secret-token' })
  })

  it('rejects access URLs that have no token fragment', () => {
    expect(tokenFromAccessUrl('http://127.0.0.1:18787/')).toBeNull()
    expect(tokenFromAccessUrl('not a url')).toBeNull()
  })

  it('reads pairingToken from secrets.json and ignores empty values', () => {
    expect(pairingTokenFromSecrets('{"pairingToken":"from-settings"}')).toBe('from-settings')
    expect(pairingTokenFromSecrets('{"pairingToken":""}')).toBeNull()
    expect(pairingTokenFromSecrets('{')).toBeNull()
  })

  it('never echoes credentials when redacting an origin', () => {
    expect(redactOrigin('https://user:pass@example.test:443/path#access_token=x')).toBe(
      'https://example.test'
    )
  })

  it('builds the same WebSocket paths the iPhone companion uses', () => {
    expect(wsUrl('http://127.0.0.1:18787', '/ws')).toBe('ws://127.0.0.1:18787/ws')
    expect(wsUrl('https://termul.example.test', '/terminal/ws')).toBe(
      'wss://termul.example.test/terminal/ws'
    )
  })

  it('loads only the iPhone tail window instead of the whole transcript', () => {
    expect(historyAfterSeq(0)).toBeNull()
    expect(historyAfterSeq(12)).toBe(0)
    expect(historyAfterSeq(HISTORY_TAIL_WINDOW + 10)).toBe(10)
    expect(historyAfterSeq(80, 80)).toBeNull()
    expect(historyAfterSeq(90, 40)).toBe(40)
  })

  it('looks in the desktop dev and prod app-data trees, never the keychain', () => {
    const roots = appDataCandidates('/Users/demo')
    expect(roots.some((path) => path.endsWith(brandCanonical().bundleIdDev))).toBe(true)
    expect(roots.some((path) => path.endsWith(brandCanonical().bundleId))).toBe(true)
    expect(
      secretsPaths('/Users/demo').every((path) => path.endsWith('remote-tunnel/secrets.json'))
    ).toBe(true)
  })

  it('keeps the named-tunnel loopback origin as the default local target', () => {
    expect(DEFAULT_LOCAL_ORIGIN).toBe('http://127.0.0.1:18787')
  })

  it('finds LAN listeners bound to all interfaces, not only 127.0.0.1', () => {
    const text = [
      'TermulMan 1234 qs  12u  IPv4 0x1  0t0  TCP *:51234 (LISTEN)',
      'vite      5678 qs  21u  IPv4 0x2  0t0  TCP 127.0.0.1:5180 (LISTEN)'
    ].join('\n')
    const ports = listenPortsFromLsof(text)
    expect(ports.named).toEqual(['51234'])
    expect(ports.wildcard).toEqual(['51234'])
    expect(ports.loopback).toEqual(['5180'])
  })

  it('reads remote-access intent without touching secrets', () => {
    expect(parseRemoteAccessIntent('{"wanted":true,"publishMode":"lan"}')).toEqual({
      wanted: true,
      publishMode: 'lan'
    })
    expect(parseRemoteAccessIntent('{')).toBeNull()
    expect(
      remoteAccessIntentPaths('/Users/demo').every((path) =>
        path.endsWith('remote-tunnel/remote-access.json')
      )
    ).toBe(true)
  })

  it('normalizes terminal {success,data} replies onto the ACP {ok,payload} shape', () => {
    expect(normalizeWsReply({ id: 't1', success: true, data: { terminals: [] } })).toMatchObject({
      ok: true,
      payload: { terminals: [] }
    })
    expect(normalizeWsReply({ id: 't2', success: false, error: 'nope', code: 'X' })).toMatchObject({
      ok: false,
      err: { code: 'X', message: 'nope' }
    })
  })

  it('prefers a live agent id and falls back to the binding runtime', () => {
    expect(pickLiveAgentId(['cursor-1', 'claude-2'], 'claude-2')).toBe('claude-2')
    expect(pickLiveAgentId(['cursor-1'], 'missing')).toBe('cursor-1')
    expect(pickLiveAgentId([], 'cursor-1')).toBe('cursor-1')
    expect(pickLiveAgentId([])).toBeNull()
  })

  it('picks an allow-once permission option the way the phone would', () => {
    expect(
      pickPermissionOption({
        options: [
          { optionId: 'reject', name: 'Reject' },
          { optionId: 'allow-once', name: 'Allow once' }
        ]
      })
    ).toBe('allow-once')
    expect(pickPermissionOption({ options: [] })).toBeNull()
  })

  it('retries subscribe without lastSeq after a stale cursor, like the iPhone', () => {
    expect(subscribePayload('s1', 12)).toEqual({ sessionId: 's1', lastSeq: 12 })
    expect(subscribePayload('s1', 12, true)).toEqual({ sessionId: 's1' })
  })

  it('finds the e2e marker in a reloaded history page', () => {
    const marker = 'e2e-ok-1'
    expect(e2ePrompt(marker)).toContain(marker)
    expect(
      historyContainsMarker(
        [{ payload: { content: [{ text: `Reply with exactly ${marker}` }] } }],
        marker
      )
    ).toBe(true)
    expect(historyContainsMarker([{ payload: { type: 'ping' } }], marker)).toBe(false)
  })

  it('picks a small text file the iPhone file browser would open', () => {
    expect(
      pickBrowsableFile([
        { name: 'src', path: '/proj/src', type: 'directory' },
        { name: 'README.md', path: '/proj/README.md', type: 'file', size: 120 }
      ])
    ).toBe('/proj/README.md')
    expect(pickBrowsableFile([{ name: 'src', path: '/proj/src', type: 'directory' }])).toBeNull()
  })
})
