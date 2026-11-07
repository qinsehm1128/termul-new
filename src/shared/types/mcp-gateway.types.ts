/** Runtime-neutral identities for the two independent MCP connections. */
export type McpGatewaySource = 'internal' | 'aggregator'

export const MCP_GATEWAY_SOURCES: readonly McpGatewaySource[] = ['internal', 'aggregator']

export const MCP_GATEWAY_CLIENT_KEYS: Record<McpGatewaySource, string> = {
  internal: 'se-mcp',
  aggregator: 'se-mcp-aggregator'
}
