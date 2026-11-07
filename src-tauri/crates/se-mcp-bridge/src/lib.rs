//! Connect an agent to one isolated Se MCP gateway over stdio.
//!
//! `--source internal` (default) exposes only Se capabilities;
//! `--source aggregator` exposes only external upstreams. Each reads its own
//! settings, port, and token; stdio client snippets contain no credentials.

pub mod bridge;
pub mod identity;
pub mod settings;

use std::path::PathBuf;

pub use identity::{
    gateway_http_client, validate_gateway_origin, verify_gateway_identity, IdentityError,
};
pub use settings::{
    GatewayIdentity, GatewaySettings, Mode, Source, GATEWAY_IDENTITY_PATH, GATEWAY_PROTOCOL,
    GATEWAY_PROTOCOL_VERSION, PROTOCOL_VERSION_HEADER, SCHEMA_VERSION, SOURCE_HEADER,
};

const USAGE: &str = "\
se-mcp — connect an agent to Se Manager's MCP gateway over stdio

USAGE:
    se-mcp [--source internal|aggregator] [--mode grouped|entry|direct] [--config <path>] [--url <url>] [--no-autostart]

OPTIONS:
    --source <source> internal (default): Se capabilities; aggregator: external upstreams
    --mode <mode>     grouped (default): <server>_tool_list / <server>_tool_call per server
                      entry: list_mcp_servers / list_mcp_tools / call_mcp_tool / get_mcp_server_status
                      direct: every tool of every server
    --config <path>   source settings file (default ~/.se-manager/mcp-gateway[-aggregator][.dev].json)
    --url <url>       same settings origin and selected mode path only
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
    let mut source = Source::Internal;
    let mut settings_path: Option<PathBuf> = None;
    let mut url = None;
    let mut autostart = true;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(None),
            "--source" => {
                let value = arguments.next().ok_or("MCP_SOURCE_REQUIRED")?;
                source = Source::parse(&value).ok_or("MCP_SOURCE_INVALID")?;
            }
            "--mode" => {
                let value = arguments.next().ok_or("MCP_MODE_REQUIRED")?;
                mode = Mode::parse(&value).ok_or("MCP_MODE_INVALID")?;
            }
            "--config" => {
                settings_path = Some(arguments.next().ok_or("--config needs a path")?.into());
            }
            "--url" => url = Some(arguments.next().ok_or("--url needs a value")?),
            "--no-autostart" => autostart = false,
            _ => return Err("MCP_ARGUMENT_INVALID".into()),
        }
    }
    let settings_path = settings_path
        .or_else(|| settings::default_path_for_source(source))
        .ok_or("cannot locate the home directory; pass --config")?;
    Ok(Some(bridge::BridgeOptions {
        settings_path,
        source,
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
    fn parses_six_source_mode_combinations_and_default_files() {
        for source in [Source::Internal, Source::Aggregator] {
            for mode in [Mode::Grouped, Mode::Entry, Mode::Direct] {
                let options = parse(args(&[
                    "--source",
                    source.as_str(),
                    "--mode",
                    mode.as_str(),
                ]))
                .unwrap()
                .unwrap();
                assert_eq!(options.source, source);
                assert_eq!(options.mode, mode);
                assert_eq!(
                    options.settings_path.file_name().unwrap(),
                    source.default_file_name()
                );
            }
        }
        let options = parse(args(&[])).unwrap().unwrap();
        assert_eq!(options.source, Source::Internal);
        assert_eq!(options.mode, Mode::Grouped);
        let explicit = parse(args(&[
            "--config",
            "/custom/settings.json",
            "--source",
            "aggregator",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(explicit.source, Source::Aggregator);
        assert_eq!(
            explicit.settings_path,
            PathBuf::from("/custom/settings.json")
        );
    }

    #[test]
    fn parser_does_not_echo_secrets_or_accept_unknown_sources() {
        for values in [
            vec!["--source", "SOURCE_SECRET"],
            vec!["--source", "Internal"],
            vec!["--source"],
            vec!["--mode", "MODE_SECRET"],
            vec!["http://URL_SECRET"],
        ] {
            let error = match parse(args(&values)) {
                Err(error) => error,
                _ => panic!("expected parser error"),
            };
            assert!(!error.contains("SECRET"));
        }
    }

    #[test]
    fn rejects_unknown_mode_and_arguments() {
        assert!(parse(args(&["--mode", "fast"])).is_err());
        assert!(parse(args(&["--port", "1"])).is_err());
        assert!(parse(args(&["--help"])).unwrap().is_none());
    }
}
