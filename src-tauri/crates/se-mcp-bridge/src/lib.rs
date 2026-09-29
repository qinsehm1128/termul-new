//! Connects an agent to Se Manager's MCP gateway.
//!
//! An agent configures one stdio MCP server, `se-mcp`, and through it reaches
//! every MCP server Se Manager aggregates. `se-mcp` reads the gateway's port
//! and token from the settings file Se Manager writes, so the agent's own
//! configuration never changes when the port does.

pub mod bridge;
pub mod settings;

use std::path::PathBuf;

pub use settings::{GatewaySettings, Mode};

const USAGE: &str = "\
se-mcp — connect an agent to Se Manager's MCP gateway over stdio

USAGE:
    se-mcp [--mode grouped|entry|direct] [--config <path>] [--no-autostart]

OPTIONS:
    --mode <mode>     grouped (default): <server>_tool_list / <server>_tool_call per server
                      entry: list_mcp_servers / list_mcp_tools / call_mcp_tool / get_mcp_server_status
                      direct: every tool of every server
    --config <path>   gateway settings file (default ~/.se-manager/mcp-gateway.json)
    --url <url>       talk to this endpoint instead of the one in the settings file
    --no-autostart    do not start the gateway when it is not running
";

/// Parse arguments, relay until stdin closes, and return the exit code.
pub fn run_cli(arguments: impl IntoIterator<Item = String>) -> i32 {
    let options = match parse(arguments) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return 0;
        }
        Err(message) => {
            eprintln!("se-mcp: {message}\n\n{USAGE}");
            return 2;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("se-mcp: cannot start the async runtime: {error}");
            return 1;
        }
    };
    match runtime.block_on(bridge::run(options)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("se-mcp: {error}");
            1
        }
    }
}

fn parse(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Option<bridge::BridgeOptions>, String> {
    let mut mode = Mode::Grouped;
    let mut settings_path: Option<PathBuf> = None;
    let mut url = None;
    let mut autostart = true;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(None),
            "--mode" => {
                let value = arguments.next().ok_or("--mode needs a value")?;
                mode = Mode::parse(&value).ok_or(format!("unknown mode `{value}`"))?;
            }
            "--config" => {
                settings_path = Some(arguments.next().ok_or("--config needs a path")?.into());
            }
            "--url" => url = Some(arguments.next().ok_or("--url needs a value")?),
            "--no-autostart" => autostart = false,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let settings_path = settings_path
        .or_else(settings::default_path)
        .ok_or("cannot locate the home directory; pass --config")?;
    Ok(Some(bridge::BridgeOptions {
        settings_path,
        mode,
        url,
        autostart,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_mode_config_and_flags() {
        let options = parse(args(&[
            "--mode",
            "entry",
            "--config",
            "/tmp/g.json",
            "--no-autostart",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(options.mode, Mode::Entry);
        assert_eq!(options.settings_path, PathBuf::from("/tmp/g.json"));
        assert!(!options.autostart);
    }

    #[test]
    fn rejects_unknown_mode_and_arguments() {
        assert!(parse(args(&["--mode", "fast"])).is_err());
        assert!(parse(args(&["--port", "1"])).is_err());
        assert!(parse(args(&["--help"])).unwrap().is_none());
    }
}
