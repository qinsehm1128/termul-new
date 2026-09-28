import { describe, expect, it } from 'vitest'
import {
  acceptAiRevision,
  createAiRevisionFence,
  parseAiAnalysisStatus,
  parseAiChannelsDocument,
  parseAiCredentialStatus,
  parseFxCapabilityStatus,
  serializeAiAnalysisStatus,
  serializeAiChannelsDocument
} from './ai-channels.types'
import analysisFixture from './fixtures/ai-analysis-status.json'
import fullFixture from './fixtures/ai-channels.document.json'
import minimalFixture from './fixtures/ai-channels.minimal.json'
import credentialFixture from './fixtures/ai-credential-status.json'
import fxFixture from './fixtures/fx-capability.json'

const CANARY = 'sk-canary-secret'

describe('ai channel contracts', () => {
  it('round-trips the shared document fixture, including optional fields', () => {
    const parsed = parseAiChannelsDocument(fullFixture)
    expect(parsed.channels[0]?.provider).toBe('vercelGateway')
    expect(parsed.channels[0]?.baseUrl).toBe('https://gateway.example.test/v1')
    expect(parsed.channels[0]?.credentialRef).toEqual({
      kind: 'keyring',
      ref: 'ai/channel/gateway',
      hasCredential: true
    })
    expect(parsed.profiles[0]?.temperature).toBe(0.5)
    expect(parsed.routes.map((route) => route.purpose)).toEqual([
      'descriptionAnalysis',
      'fxRuntime'
    ])
    expect(serializeAiChannelsDocument(parsed)).toEqual(parsed)
    expect(parseAiChannelsDocument(serializeAiChannelsDocument(parsed))).toEqual(parsed)
  })

  it('omits optional endpoint and sampling fields', () => {
    const parsed = parseAiChannelsDocument(minimalFixture)
    expect(parsed.channels[0]?.baseUrl).toBeUndefined()
    expect(parsed.profiles[0]?.maxOutputTokens).toBeUndefined()
    expect(parsed.profiles[0]?.temperature).toBeUndefined()
    const serialized = serializeAiChannelsDocument(parsed)
    expect(serialized.channels[0]).not.toHaveProperty('baseUrl')
    expect(serialized.profiles[0]).not.toHaveProperty('maxOutputTokens')
    expect(serialized.profiles[0]).not.toHaveProperty('temperature')
    expect(parseAiChannelsDocument(serialized)).toEqual(parsed)
  })

  it('fails closed on an invalid provider, endpoint, or id without echoing secrets', () => {
    const document = structuredClone(fullFixture) as Record<string, unknown>
    const channels = document.channels as Array<Record<string, unknown>>
    channels[0].provider = 'azure'
    expect(() => parseAiChannelsDocument(document)).toThrow('AI provider is invalid')

    channels[0].provider = 'custom'
    channels[0].baseUrl = `https://user:${CANARY}@example.test/v1`
    expect(() => parseAiChannelsDocument(document)).toThrow('AI endpoint is invalid')
    try {
      parseAiChannelsDocument(document)
    } catch (error) {
      expect(error instanceof Error ? error.message : '').not.toContain(CANARY)
    }

    channels[0].baseUrl = 'http://192.168.1.20/v1?api_key=sk-live'
    expect(() => parseAiChannelsDocument(document)).toThrow('AI endpoint is invalid')

    delete channels[0].baseUrl
    expect(() => parseAiChannelsDocument(document)).toThrow('AI endpoint is invalid')

    channels[0].provider = 'ollama'
    channels[0].baseUrl = 'http://127.0.0.1:11434/v1'
    channels[0].id = 'not an id'
    expect(() => parseAiChannelsDocument(document)).toThrow('AI id is invalid')
  })

  it('rejects a credential field on the renderer-facing document', () => {
    const document = structuredClone(fullFixture) as {
      channels: Array<Record<string, unknown>>
    }
    document.channels[0].apiKey = CANARY
    expect(() => parseAiChannelsDocument(document)).toThrow(
      'AI contract contains a forbidden credential field'
    )
    try {
      parseAiChannelsDocument(document)
    } catch (error) {
      expect(String(error)).not.toContain(CANARY)
    }
  })

  it('parses credential, analysis, and Fx status fixtures', () => {
    expect(parseAiCredentialStatus(credentialFixture)).toEqual({
      channelId: 'gateway',
      kind: 'keyring',
      ref: 'ai/channel/gateway',
      hasCredential: false,
      state: 'missing'
    })
    expect(() =>
      parseAiCredentialStatus({ ...credentialFixture, hasCredential: true, state: 'missing' })
    ).toThrow('AI contract document is invalid')

    const analysis = parseAiAnalysisStatus(analysisFixture)
    expect(serializeAiAnalysisStatus(analysis)).toEqual(analysis)
    expect(analysis.errorCode).toBeUndefined()
    expect(serializeAiAnalysisStatus(analysis)).not.toHaveProperty('errorCode')

    const fx = parseFxCapabilityStatus(fxFixture)
    expect(fx.selectedRuntime).toBe('wasm')
    expect(fx.fallback).toBe('aiRouter')
    expect(() => parseFxCapabilityStatus({ ...fxFixture, selectedRuntime: 'native' })).toThrow(
      'AI contract document is invalid'
    )
  })

  it('keeps a strict revision fence', () => {
    const fence = createAiRevisionFence()
    acceptAiRevision(fence, 1)
    expect(() => acceptAiRevision(fence, 1)).toThrow('AI contract revision is stale')
    acceptAiRevision(fence, 2)
    expect(fence.lastAccepted).toBe(2)
  })
})
