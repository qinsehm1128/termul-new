//! Retired environment-configured MCP child entrypoint.

pub fn run_mcp_core_process() -> i32 {
    eprintln!(
        "MCP gateway requires --config; use se-mcp --source internal or --source aggregator."
    );
    2
}
