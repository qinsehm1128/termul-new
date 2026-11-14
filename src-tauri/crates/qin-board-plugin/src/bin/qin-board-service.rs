//! qin-board-service: loopback WebSocket ⇄ qin-code api-bridge relay.
//!
//! Prints exactly one stdout line `QIN_BOARD_ENDPOINT {"port":P,"token":"T"}`.
//! That line is the only place the token is ever written; diagnostics go to
//! stderr and never include it.
//!
//! Unix only: the qin-code api bridge it relays to listens on a Unix socket.
//! Other platforms get a binary that says so, which keeps the workspace
//! building there.
#[cfg(unix)]
#[path = "../service/mod.rs"]
mod service;

#[cfg(unix)]
fn main() {
    service::server::run();
}

#[cfg(not(unix))]
fn main() {
    eprintln!(
        "qin-board-service: not supported on this platform (the qin-code api bridge is a Unix socket)"
    );
    std::process::exit(1);
}
