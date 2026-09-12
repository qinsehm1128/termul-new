/**
 * Render the Se memory-MCP invocation (an argv array produced by the host) as
 * pasteable client configuration blocks.
 *
 * Two canonical targets, verified against the clients' own docs:
 *
 * - **Claude Desktop / Claude Code / generic mcpServers JSON** — the wrapper
 *   key `mcpServers` with one entry per server (`command` + `args`). Claude
 *   Code's `mcp add-json` consumes the entry object (inside the wrapper);
 *   Claude Desktop's `claude_desktop_config.json` consumes the whole block.
 * - **Codex `config.toml`** — a `[mcp_servers.<key>]` table with `command` and
 *   `args` keys. Codex's own CI home config references that table name.
 *
 * Pure string building so both shapes are unit-testable without a file system.
 */

export type McpClientConfigFormat = 'json' | 'toml'

/** Derive a stable, paste-safe server key from the project folder name. */
export function memoryMcpServerKey(projectName: string): string {
  const slug = projectName
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return slug ? `se-memory-${slug}` : 'se-memory'
}

/** `#rrggbb`-style guard is unnecessary here: argv entries are plain strings. */
export function buildMcpClientConfig(
  invocation: string[],
  format: McpClientConfigFormat,
  projectName: string
): string {
  const key = memoryMcpServerKey(projectName)
  const [command, ...args] = invocation
  if (format === 'json') {
    return JSON.stringify(
      { mcpServers: { [key]: { command, ...(args.length > 0 ? { args } : {}) } } },
      null,
      2
    )
  }
  return [
    `[mcp_servers.${key}]`,
    `command = ${tomlString(command)}`,
    ...(args.length > 0 ? [`args = [${args.map(tomlString).join(', ')}]`] : []),
    ''
  ].join('\n')
}

/** Basic TOML basic-string escaping (RFC: backslash escapes, quote, controls). */
function tomlString(value: string): string {
  return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
}
