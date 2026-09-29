//! `se-mcp`: the stdio MCP client an agent configures to reach every MCP
//! server Se Manager aggregates. See `se_mcp_bridge` for the relay itself.

fn main() {
    std::process::exit(se_mcp_bridge::run_cli(std::env::args().skip(1)));
}
