//! Loopback WebSocket listener: authenticates each client with the per-launch
//! token, then hands the socket to a session.
use super::{bridge, session};
use qin_board_plugin::token;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

fn parse_port() -> Result<u16, String> {
    let mut args = std::env::args().skip(1);
    let mut port = 0u16;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                port = args
                    .next()
                    .ok_or("--port needs a value")?
                    .parse()
                    .map_err(|_| "--port must be 0..65535")?;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(port)
}

fn unauthorized() -> ErrorResponse {
    let mut response = ErrorResponse::new(Some("unauthorized".to_owned()));
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    response
}

// The handshake callback's error type is fixed by tungstenite's Callback trait.
#[allow(clippy::result_large_err)]
fn handle(stream: TcpStream, token: Arc<String>) {
    if stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).is_err() {
        return;
    }
    // Origin is intentionally not checked: the plugin web view is an opaque
    // origin (`Origin: null`). The per-launch token is the only credential.
    let callback = |request: &Request, response: Response| {
        if token::query_authorized(&token, request.uri().query()) {
            Ok(response)
        } else {
            Err(unauthorized())
        }
    };
    match tungstenite::accept_hdr(stream, callback) {
        Ok(ws) => session::run(ws),
        Err(error) => eprintln!("qin-board-service: handshake rejected: {error}"),
    }
}

/// Bind the loopback listener, announce the endpoint on stdout, and serve
/// clients until the process is stopped.
pub fn run() {
    let port = match parse_port() {
        Ok(port) => port,
        Err(error) => {
            eprintln!("qin-board-service: {error}");
            std::process::exit(2);
        }
    };
    let token = match token::generate() {
        Ok(token) => Arc::new(token),
        Err(error) => {
            eprintln!("qin-board-service: random token unavailable: {error}");
            std::process::exit(1);
        }
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("qin-board-service: bind 127.0.0.1:{port} failed: {error}");
            std::process::exit(1);
        }
    };
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let announcement = serde_json::json!({"port":bound,"token":token.as_str()});
    {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "QIN_BOARD_ENDPOINT {announcement}");
        let _ = stdout.flush();
    }
    eprintln!(
        "qin-board-service: listening on 127.0.0.1:{bound}, api socket {}",
        bridge::socket_path().display()
    );
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let token = Arc::clone(&token);
        let spawned = std::thread::Builder::new()
            .name("qin-board-ws".into())
            .spawn(move || handle(stream, token));
        if let Err(error) = spawned {
            eprintln!("qin-board-service: could not spawn connection thread: {error}");
        }
    }
}
