// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Tombstone only: never let an old MCP client fall through to the GUI or
    // another process role. Do not echo arguments (they may contain secrets).
    if std::env::args_os()
        .skip(1)
        .any(|argument| argument == "--memory-mcp-server")
    {
        eprintln!("se-memory has been removed; migrate to se-mcp (session-memory).");
        std::process::exit(2);
    }

    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments
        .iter()
        .skip(1)
        .any(|argument| argument == "--mcp-core")
    {
        // Only a source-bound settings file may start a gateway. A missing
        // value must fail closed rather than becoming another process role.
        let settings = arguments
            .iter()
            .position(|argument| argument == "--config")
            .and_then(|index| arguments.get(index + 1))
            .filter(|value| !value.to_string_lossy().starts_with("--"));
        std::process::exit(match settings {
            Some(path) => se_manager_lib::mcp_core::service::run_service(path.into()),
            None => se_manager_lib::mcp_core::run_mcp_core_process(),
        });
    }

    let observer_build_id = option_env!("TERMUL_GUI_BUILD_ID").unwrap_or(env!("CARGO_PKG_VERSION"));
    if let Some(code) = se_ghostty_observer::dispatch(observer_build_id) {
        std::process::exit(code);
    }
    se_pty::screen::configure_observer(cfg!(feature = "ghostty-vt"), observer_build_id);

    if let Some(role) = std::env::args()
        .skip(1)
        .find_map(|argument| match argument.as_str() {
            "--terminal-core" => Some(se_manager_lib::core::CoreRole::TerminalCore),
            "--acp-core" => Some(se_manager_lib::core::CoreRole::AcpCore),
            "--plugin-core" => Some(se_manager_lib::core::CoreRole::PluginCore),
            _ => None,
        })
    {
        std::process::exit(se_manager_lib::core::run_core_process(role));
    }

    // Self-spawned `--internal-mcp-plan-server` child: the agent spawns this
    // binary (current_exe) as the injected `McpServer::Stdio` for the
    // host-injected `plan` tool. Branch BEFORE any Tauri/log setup so
    // the child never initializes the app / plugins / sinks — it just runs
    // an rmcp MCP server over stdio + forwards calls to the parent's TCP
    // listener. See `acp::host_mcp::child` + spec `spec-acp-host-todo-plan-tool.md`.
    if se_manager_lib::host_mcp::is_child_invocation() {
        std::process::exit(se_manager_lib::host_mcp::child::run());
    }

    // Seed a default RUST_LOG so module-level overrides keep working, e.g.:
    //   RUST_LOG=trace npm run dev
    //   RUST_LOG=se_manager_lib=debug npm run dev
    // The global logger itself (file sink in release, console in debug) is
    // installed by tauri-plugin-log inside `run()`; its level floor is set
    // there (info in release, debug in debug builds).
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var(
            "RUST_LOG",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "info"
            },
        );
    }

    se_manager_lib::run()
}
