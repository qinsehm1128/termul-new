//! A deliberately small managed local service. No runtime, threads or dependencies.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};
fn serve(mut stream: TcpStream, start: Instant) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    let mut request = [0; 4096];
    let n = stream.read(&mut request)?;
    let request = String::from_utf8_lossy(&request[..n]);
    let (status, body) = if request.starts_with("GET /ping ") {
        (
            "200 OK",
            format!(
                "{{\"ok\":true,\"uptimeMs\":{},\"pid\":{}}}",
                start.elapsed().as_millis(),
                std::process::id()
            ),
        )
    } else if request.starts_with("OPTIONS ") {
        ("204 No Content", String::new())
    } else {
        ("404 Not Found", "{\"ok\":false}".into())
    };
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Private-Network: true\r\nConnection: close\r\n\r\n{body}", body.len())?;
    println!(
        "request {} -> {status}",
        request.lines().next().unwrap_or("empty")
    );
    Ok(())
}
fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let port: u16 = if args.next().as_deref() == Some("--port") {
        args.next()
    } else {
        std::env::var("SE_ACTIVITY_PORT").ok()
    }
    .unwrap_or_else(|| "0".into())
    .parse()
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    println!("SE_ACTIVITY_PORT={}", listener.local_addr()?.port());
    std::io::stdout().flush()?;
    eprintln!("Activity Studio service pid={} ready", std::process::id());
    let start = Instant::now();
    listener.set_nonblocking(true)?;
    #[cfg(unix)]
    let owner = parent_pid();
    loop {
        // Unix does not automatically kill children when their owner aborts.
        // Exit after reparenting; the host still owns normal stop/upgrade/disable.
        #[cfg(unix)]
        if owner <= 1 || parent_pid() != owner {
            eprintln!("Activity Studio owner exited; stopping service");
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(e) = serve(stream, start) {
                    eprintln!("request error: {e}");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
#[cfg(unix)]
fn parent_pid() -> i32 {
    unsafe extern "C" {
        fn getppid() -> i32;
    }
    // SAFETY: POSIX getppid takes no pointers and has no preconditions.
    unsafe { getppid() }
}
