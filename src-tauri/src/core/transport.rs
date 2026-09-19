//! Transport-agnostic Core IPC streams.
//!
//! Framing and handshake live in [`super::ipc`]. This module owns the local
//! byte pipe: Unix sockets on Unix, named pipes on Windows. Client and server
//! types implement `AsyncRead` + `AsyncWrite` by delegation so the rest of the
//! Core stack never mentions a platform stream.

use super::ipc::{CoreEndpoint, CoreError, CoreRole};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};

/// Client-side Core IPC stream.
pub enum CoreStream {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    #[cfg(windows)]
    Pipe(NamedPipeClient),
}

/// [`connect_core`] return type; alias of [`CoreStream`].
pub type CoreClientStream = CoreStream;

/// Server-side stream accepted from [`CoreListener`].
pub enum CoreServerStream {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    #[cfg(windows)]
    Pipe(NamedPipeServer),
}

/// Read half after [`CoreStream::into_split`] / [`CoreServerStream::into_split`].
pub enum CoreReadHalf {
    #[cfg(unix)]
    Unix(tokio::net::unix::OwnedReadHalf),
    #[cfg(windows)]
    Client(tokio::io::ReadHalf<NamedPipeClient>),
    #[cfg(windows)]
    Server(tokio::io::ReadHalf<NamedPipeServer>),
}

/// Write half after [`CoreStream::into_split`] / [`CoreServerStream::into_split`].
pub enum CoreWriteHalf {
    #[cfg(unix)]
    Unix(tokio::net::unix::OwnedWriteHalf),
    #[cfg(windows)]
    Client(tokio::io::WriteHalf<NamedPipeClient>),
    #[cfg(windows)]
    Server(tokio::io::WriteHalf<NamedPipeServer>),
}

/// Listening endpoint for a Core role.
pub enum CoreListener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    #[cfg(windows)]
    Pipe {
        name: String,
        server: Option<NamedPipeServer>,
    },
}

macro_rules! delegate_async_read {
    ($ty:ty => $($cfg:meta => $variant:ident),+ $(,)?) => {
        impl AsyncRead for $ty {
            fn poll_read(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                match self.get_mut() {
                    $(
                        #[cfg($cfg)]
                        Self::$variant(inner) => Pin::new(inner).poll_read(cx, buf),
                    )+
                    #[cfg(not(any(unix, windows)))]
                    _ => unreachable!(),
                }
            }
        }
    };
}

macro_rules! delegate_async_write {
    ($ty:ty => $($cfg:meta => $variant:ident),+ $(,)?) => {
        impl AsyncWrite for $ty {
            fn poll_write(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                match self.get_mut() {
                    $(
                        #[cfg($cfg)]
                        Self::$variant(inner) => Pin::new(inner).poll_write(cx, buf),
                    )+
                    #[cfg(not(any(unix, windows)))]
                    _ => unreachable!(),
                }
            }

            fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                match self.get_mut() {
                    $(
                        #[cfg($cfg)]
                        Self::$variant(inner) => Pin::new(inner).poll_flush(cx),
                    )+
                    #[cfg(not(any(unix, windows)))]
                    _ => unreachable!(),
                }
            }

            fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                match self.get_mut() {
                    $(
                        #[cfg($cfg)]
                        Self::$variant(inner) => Pin::new(inner).poll_shutdown(cx),
                    )+
                    #[cfg(not(any(unix, windows)))]
                    _ => unreachable!(),
                }
            }
        }
    };
}

delegate_async_read!(CoreStream => unix => Unix, windows => Pipe);
delegate_async_write!(CoreStream => unix => Unix, windows => Pipe);
delegate_async_read!(CoreServerStream => unix => Unix, windows => Pipe);
delegate_async_write!(CoreServerStream => unix => Unix, windows => Pipe);
delegate_async_read!(CoreReadHalf => unix => Unix, windows => Client, windows => Server);
delegate_async_write!(CoreWriteHalf => unix => Unix, windows => Client, windows => Server);

impl CoreStream {
    pub fn into_split(self) -> (CoreReadHalf, CoreWriteHalf) {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => {
                let (reader, writer) = stream.into_split();
                (CoreReadHalf::Unix(reader), CoreWriteHalf::Unix(writer))
            }
            #[cfg(windows)]
            Self::Pipe(stream) => {
                let (reader, writer) = tokio::io::split(stream);
                (CoreReadHalf::Client(reader), CoreWriteHalf::Client(writer))
            }
            #[cfg(not(any(unix, windows)))]
            _ => unreachable!(),
        }
    }
}

impl CoreServerStream {
    pub fn into_split(self) -> (CoreReadHalf, CoreWriteHalf) {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => {
                let (reader, writer) = stream.into_split();
                (CoreReadHalf::Unix(reader), CoreWriteHalf::Unix(writer))
            }
            #[cfg(windows)]
            Self::Pipe(stream) => {
                let (reader, writer) = tokio::io::split(stream);
                (CoreReadHalf::Server(reader), CoreWriteHalf::Server(writer))
            }
            #[cfg(not(any(unix, windows)))]
            _ => unreachable!(),
        }
    }
}

/// Connect to a Core IPC endpoint.
///
/// Unix: `UnixStream::connect`.
/// Windows: `ClientOptions::open` with `ERROR_PIPE_BUSY` retries (25ms backoff,
/// ~5s cap).
pub async fn connect_core(endpoint: &CoreEndpoint) -> Result<CoreClientStream, CoreError> {
    #[cfg(unix)]
    {
        use tokio::net::UnixStream;

        let path = endpoint.as_path().ok_or(CoreError::UnsupportedPlatform)?;
        let stream = UnixStream::connect(path).await.map_err(CoreError::from)?;
        Ok(CoreStream::Unix(stream))
    }
    #[cfg(windows)]
    {
        connect_named_pipe(endpoint).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = endpoint;
        Err(CoreError::UnsupportedPlatform)
    }
}

#[cfg(windows)]
const PIPE_BUSY_RETRY: Duration = Duration::from_millis(25);
#[cfg(windows)]
const PIPE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(windows)]
fn pipe_name(endpoint: &CoreEndpoint) -> Result<&str, CoreError> {
    match endpoint {
        CoreEndpoint::NamedPipe(name) => Ok(name),
    }
}

#[cfg(windows)]
fn is_pipe_busy(error: &io::Error) -> bool {
    error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32)
}

#[cfg(windows)]
async fn connect_named_pipe(endpoint: &CoreEndpoint) -> Result<CoreClientStream, CoreError> {
    let name = pipe_name(endpoint)?;
    let deadline = tokio::time::Instant::now() + PIPE_BUSY_TIMEOUT;
    loop {
        match ClientOptions::new().open(name) {
            Ok(client) => return Ok(CoreStream::Pipe(client)),
            Err(error) if is_pipe_busy(&error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(error.into());
                }
                tokio::time::sleep(PIPE_BUSY_RETRY).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Listen for Core IPC connections on `endpoint`.
///
/// Unix: bind a Unix socket and chmod `0600` (owner-only), matching the
/// historical `run_*_core` listener setup.
///
/// Windows: create the first named-pipe instance with `first_pipe_instance(true)`.
/// Tokio applies the default security descriptor, which is intended to grant
/// access to the current user only (no custom DACL is installed).
#[allow(clippy::unused_async)]
pub async fn listen_core(
    endpoint: &CoreEndpoint,
    role: CoreRole,
) -> Result<CoreListener, CoreError> {
    let _ = role;
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::UnixListener;

        let path = endpoint.as_path().ok_or(CoreError::UnsupportedPlatform)?;
        let listener = UnixListener::bind(path).map_err(CoreError::from)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(CoreError::from)?;
        Ok(CoreListener::Unix(listener))
    }
    #[cfg(windows)]
    {
        let name = pipe_name(endpoint)?.to_string();
        // `first_pipe_instance(true)` makes this process the unique server for
        // the pipe. Tokio's default security descriptor is the current-user ACL
        // we intend; we do not install a custom DACL here.
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .map_err(CoreError::from)?;
        Ok(CoreListener::Pipe {
            name,
            server: Some(server),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = endpoint;
        Err(CoreError::UnsupportedPlatform)
    }
}

impl CoreListener {
    /// Accept one client. On Windows this waits for `connect()`, then creates
    /// the next pipe instance so another client can attach while the accepted
    /// one is served.
    pub async fn accept(&mut self) -> Result<CoreServerStream, CoreError> {
        match self {
            #[cfg(unix)]
            Self::Unix(listener) => {
                let (stream, _) = listener.accept().await.map_err(CoreError::from)?;
                Ok(CoreServerStream::Unix(stream))
            }
            #[cfg(windows)]
            Self::Pipe { name, server } => {
                let mut connected = match server.take() {
                    Some(connected) => connected,
                    None => {
                        return Err(CoreError::Io(
                            "named pipe listener has no waiting instance".to_string(),
                        ))
                    }
                };
                if let Err(error) = connected.connect().await {
                    *server = Some(connected);
                    return Err(error.into());
                }
                let next = ServerOptions::new().create(name).map_err(CoreError::from)?;
                *server = Some(next);
                Ok(CoreServerStream::Pipe(connected))
            }
            #[cfg(not(any(unix, windows)))]
            _ => Err(CoreError::UnsupportedPlatform),
        }
    }
}
