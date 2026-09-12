/**
 * Render the Se memory-MCP invocation (an argv array produced by the host) as
 * pasteable client configuration blocks.
 *
 * The universal server serves EVERY indexed project: the client calls its
 * `memory_projects` tool to list them and passes a `project` selector to the
 * read tools — so the client-side config is one fixed server named
 * `se-memory` and needs no per-project entries.
 *
 * Two canonical targets, verified against the clients' own docs:
 *
 * - **Claude Desktop / Claude Code / generic mcpServers JSON** — the wrapper
 *   key `mcpServers` with one entry per server (`command` + `args`). Claude
 *   Code's `mcp add-json` consumes the entry object (inside the wrapper);
 *   Claude Desktop's `claude_desktop_config.json` consumes the whole block.
 * - **Codex `config.toml`** — a `[mcp_servers.<key>]` table with `command` and
 *   `args` keys (the shape codex's own CI home config references).
 *
 * Pure string building so both shapes are unit-testable without a file system.
 */

export type McpClientConfigFormat = 'json' | 'toml'

const SERVER_KEY = 'se-memory'

/** `#rrggbb`-style guard is unnecessary here: argv entries are plain strings. */
export function buildMcpClientConfig(invocation: string[], format: McpClientConfigFormat): string {
  const [command, ...args] = invocation
  if (format === 'json') {
    return JSON.stringify(
      { mcpServers: { [SERVER_KEY]: { command, ...(args.length > 0 ? { args } : {}) } } },
      null,
      2
    )
  }
  return [
    `[mcp_servers.${SERVER_KEY}]`,
    `command = ${tomlString(command)}`,
    ...(args.length > 0 ? [`args = [${args.map(tomlString).join(', ')}]`] : []),
    ''
  ].join('\n')
}

/** Basic TOML basic-string escaping (backslash escapes, quote, controls). */
function tomlString(value: string): string {
  return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
}
