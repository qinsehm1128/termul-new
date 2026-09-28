import { describe, expect, it } from 'vitest'
import draftFixture from './fixtures/mcp-analysis-draft.json'
import callFixture from './fixtures/mcp-facade-call.json'
import catalogFixture from './fixtures/mcp-facade-catalog.json'
import metadataFixture from './fixtures/mcp-metadata.document.json'
import {
  acceptMcpMetadataRevision,
  createMcpMetadataRevisionFence,
  facadeToolNames,
  type McpFacadeCatalog,
  parseMcpAnalysisDraft,
  parseMcpFacadeCatalog,
  parseMcpFacadeListQuery,
  parseMcpFacadeToolCall,
  parseMcpMetadataDocument,
  requireCurrentCatalogRevision,
  serializeMcpAnalysisDraft,
  serializeMcpFacadeCatalog
} from './mcp-facade.types'

const CANARY = 'sk-canary-secret'

describe('mcp facade contracts', () => {
  it('round-trips catalog fields and omits inputSchema when it is absent', () => {
    const catalog = parseMcpFacadeCatalog(catalogFixture)
    expect(catalog.serverId).toBe('dbx')
    expect(catalog.catalogRevision).toBe(18)
    expect(catalog.tools[0]?.inputSchema).toEqual({ type: 'object' })
    expect(catalog.failures).toEqual([])
    expect(serializeMcpFacadeCatalog(catalog)).toEqual(catalog)

    const lightweight = structuredClone(catalogFixture) as {
      tools: Array<Record<string, unknown>>
    }
    delete lightweight.tools[0].inputSchema
    const parsed = parseMcpFacadeCatalog(lightweight)
    expect(parsed.tools[0]).not.toHaveProperty('inputSchema')
    expect(serializeMcpFacadeCatalog(parsed).tools[0]).not.toHaveProperty('inputSchema')
  })

  it('rejects invalid ids, stale revisions, and credential fields', () => {
    expect(facadeToolNames('dbx')).toEqual({ list: 'dbx_tool_list', call: 'dbx_tool_call' })
    expect(() => facadeToolNames('../dbx')).toThrow('MCP facade id is invalid')

    const catalog = structuredClone(catalogFixture) as { tools: Array<Record<string, unknown>> }
    catalog.tools[0].name = 'has space'
    expect(() => parseMcpFacadeCatalog(catalog)).toThrow('MCP facade id is invalid')

    catalog.tools[0].name = 'mongo_find_documents'
    catalog.tools[0].token = CANARY
    expect(() => parseMcpFacadeCatalog(catalog)).toThrow(
      'MCP facade contains a forbidden credential field'
    )
    try {
      parseMcpFacadeCatalog(catalog)
    } catch (error) {
      expect(String(error)).not.toContain(CANARY)
    }

    expect(() => requireCurrentCatalogRevision(17, 18)).toThrow(
      'MCP facade catalog revision is stale'
    )
    expect(() => requireCurrentCatalogRevision(undefined, 18)).not.toThrow()

    const isolated = structuredClone(catalogFixture) as McpFacadeCatalog
    isolated.failures = [{ serverId: 'other', code: 'timedOut', message: 'upstream timed out' }]
    expect(() => parseMcpFacadeCatalog(isolated)).toThrow('MCP facade document is invalid')
    isolated.failures = [{ serverId: 'dbx', code: 'timedOut', message: 'upstream timed out' }]
    expect(parseMcpFacadeCatalog(isolated).failures[0]?.code).toBe('timedOut')
  })

  it('parses a tool call without requiring a catalog revision', () => {
    const call = parseMcpFacadeToolCall(callFixture)
    expect(call.catalogRevision).toBeUndefined()
    expect(call.arguments).toEqual({ collection: 'docs' })
    const pinned = parseMcpFacadeToolCall({ ...callFixture, catalogRevision: 18 })
    expect(pinned.catalogRevision).toBe(18)
    expect(parseMcpFacadeListQuery({ query: 'find', limit: 5 })).toEqual({
      query: 'find',
      limit: 5,
      includeSchema: false
    })
  })

  it('keeps analysis drafts untrusted and metadata free of safety authority', () => {
    const draft = parseMcpAnalysisDraft(draftFixture)
    expect(draft.trust).toBe('untrusted')
    expect(draft.tools[0]?.confidence).toBe(0.25)
    expect(serializeMcpAnalysisDraft(draft)).toEqual(draft)
    expect(() => parseMcpAnalysisDraft({ ...draftFixture, trust: 'approved' })).toThrow(
      'MCP metadata contract is invalid'
    )

    const metadata = parseMcpMetadataDocument(metadataFixture)
    expect(metadata.servers[0]?.tools[0]).not.toHaveProperty('destructive')
    expect(metadata.servers[0]?.tools[0]).not.toHaveProperty('allowed')
    expect(() =>
      parseMcpMetadataDocument({
        ...metadataFixture,
        servers: [
          {
            ...metadataFixture.servers[0],
            tools: [{ ...metadataFixture.servers[0].tools[0], destructive: false }]
          }
        ]
      })
    ).toThrow('MCP metadata contract is invalid')

    const fence = createMcpMetadataRevisionFence()
    acceptMcpMetadataRevision(fence, metadata.revision)
    expect(() => acceptMcpMetadataRevision(fence, metadata.revision)).toThrow(
      'MCP metadata revision is stale'
    )
  })
})
