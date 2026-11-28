//! The sole process allowed to load Ghostty FFI. Stdio is a private protocol,
//! never a PTY; there are no callbacks into PTY input or host desktop services.
use base64::{engine::general_purpose::STANDARD, Engine};
use se_ghostty_vt::Terminal;
use se_screen_protocol::*;
use std::io::{Read, Write};

/// Host-owned terminal operations the protocol drives. Kept behind a trait so
/// the control-channel lifecycle (EOF, parent death) is testable without the
/// native Ghostty build.
trait ScreenSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>>;
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), Box<dyn std::error::Error>>;
    fn tail(
        &mut self,
        max_lines: usize,
        max_bytes: usize,
    ) -> Result<(String, u16), Box<dyn std::error::Error>>;
}

struct NativeSink(Terminal);

impl ScreenSink for NativeSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        self.0.write(bytes).map_err(|_| "native write".into())
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), Box<dyn std::error::Error>> {
        self.0
            .resize(cols, rows, 0, 0)
            .map_err(|_| "native resize".into())
    }

    fn tail(
        &mut self,
        max_lines: usize,
        max_bytes: usize,
    ) -> Result<(String, u16), Box<dyn std::error::Error>> {
        let screen = self
            .0
            .screen_tail(max_lines, max_bytes)
            .map_err(|_| "native screen read")?;
        Ok((screen.text, screen.rows))
    }
}

/// Serve the private protocol until the control channel closes.
///
/// Exits `Ok(())` on `UnexpectedEof`: the owner (or its death) closed stdin, so
/// no further request can arrive and the process must not linger as an orphan.
fn serve<R, W, F>(
    build_id: &str,
    mut input: R,
    mut output: W,
    make_sink: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    R: Read,
    W: Write,
    F: FnOnce(u16, u16) -> Result<Box<dyn ScreenSink>, Box<dyn std::error::Error>>,
{
    write_frame(
        &mut output,
        &Response::Hello {
            protocol_version: PROTOCOL_VERSION,
            observer_build_id: build_id.into(),
            ghostty_revision: GHOSTTY_REVISION.into(),
        },
    )?;
    let Request::Init { cols, rows } = read_frame(&mut input)? else {
        return Err("initialization required".into());
    };
    if !valid_grid(cols, rows) {
        return Err("grid limit".into());
    }
    let mut sink = make_sink(cols, rows)?;
    let mut applied_seq = 0;
    write_frame(&mut output, &Response::Ack { applied_seq })?;
    loop {
        let request: Request = match read_frame(&mut input) {
            Ok(request) => request,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                eprintln!(
                    "[ghostty-observer] stable_code=OBSERVER_EXIT reason=control_eof applied_seq={applied_seq}"
                );
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        match request {
            Request::Output {
                event_seq,
                data_base64,
            } => {
                if event_seq != applied_seq + 1 {
                    return Err("sequence violation".into());
                }
                let bytes = STANDARD.decode(data_base64)?;
                sink.write(&bytes)?;
                applied_seq = event_seq;
                write_frame(&mut output, &Response::Ack { applied_seq })?;
            }
            Request::Resize {
                event_seq,
                cols,
                rows,
            } => {
                if event_seq != applied_seq + 1 || !valid_grid(cols, rows) {
                    return Err("resize violation".into());
                }
                sink.resize(cols, rows)?;
                applied_seq = event_seq;
                write_frame(&mut output, &Response::Ack { applied_seq })?;
            }
            Request::Read {
                request_id,
                barrier_seq,
                max_lines,
            } => {
                if barrier_seq != applied_seq || max_lines == 0 || max_lines > MAX_LINES {
                    return Err("read barrier violation".into());
                }
                let (text, rows) = sink.tail(max_lines, MAX_TEXT_BYTES)?;
                let truncated =
                    usize::from(rows) > max_lines || text.len() >= MAX_TEXT_BYTES.saturating_sub(3);
                if text.len() > MAX_TEXT_BYTES {
                    return Err("native text limit".into());
                }
                write_frame(
                    &mut output,
                    &Response::Reply {
                        request_id,
                        applied_seq,
                        text,
                        truncated,
                    },
                )?;
            }
            Request::Init { .. } => return Err("duplicate initialization".into()),
        }
    }
}

pub fn run_with_build_id(build_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !se_ghostty_vt::NATIVE_AVAILABLE {
        return Err("native unavailable".into());
    }
    spawn_parent_watchdog();
    let input = std::io::stdin().lock();
    let output = std::io::stdout().lock();
    serve(build_id, input, output, |cols, rows| {
        let terminal = Terminal::new(cols, rows)
            .map_err(|_| -> Box<dyn std::error::Error> { "native initialization".into() })?;
        Ok(Box::new(NativeSink(terminal)) as Box<dyn ScreenSink>)
    })
}

/// The owner holds our stdin write end, so a normal owner exit shows up as EOF.
/// This covers the pathological case where the owner dies while another process
/// still holds that pipe open: reparenting is then the only available signal.
#[cfg(unix)]
fn spawn_parent_watchdog() {
    let parent = unsafe { libc::getppid() } as u32;
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let current = unsafe { libc::getppid() } as u32;
        if parent_changed(parent, current) {
            eprintln!(
                "[ghostty-observer] stable_code=OBSERVER_EXIT reason=parent_gone parent={parent} current={current}"
            );
            std::process::exit(0);
        }
    });
}
#[cfg(not(unix))]
fn spawn_parent_watchdog() {}

fn parent_changed(original: u32, current: u32) -> bool {
    original != current
}

/// Must run before any application initialization; arguments are exact.
pub fn dispatch(build_id: &str) -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_none_or(|arg| arg != "--ghostty-vt-observer")
    {
        return None;
    }
    if args.len() != 1 {
        return Some(64);
    }
    Some(if run_with_build_id(build_id).is_ok() {
        0
    } else {
        70
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSink;

    impl ScreenSink for FakeSink {
        fn write(&mut self, _bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
        fn resize(&mut self, _cols: u16, _rows: u16) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
        fn tail(
            &mut self,
            _max_lines: usize,
            _max_bytes: usize,
        ) -> Result<(String, u16), Box<dyn std::error::Error>> {
            Ok((String::from("screen"), 1))
        }
    }

    fn fake(_cols: u16, _rows: u16) -> Result<Box<dyn ScreenSink>, Box<dyn std::error::Error>> {
        Ok(Box::new(FakeSink))
    }

    fn init_frame(cols: u16, rows: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &Request::Init { cols, rows }).unwrap();
        bytes
    }

    /// Regression: closing the control channel ends the process loop cleanly
    /// instead of leaving an observer waiting on a pipe nobody writes to.
    #[test]
    fn control_channel_eof_exits_cleanly() {
        let mut output = Vec::new();
        let result = serve(
            "test-build",
            std::io::Cursor::new(init_frame(80, 24)),
            &mut output,
            fake,
        );
        assert!(result.is_ok());
        let mut frames = output.as_slice();
        let hello: Response = read_frame(&mut frames).unwrap();
        assert!(matches!(hello, Response::Hello { .. }));
        let ack: Response = read_frame(&mut frames).unwrap();
        assert!(matches!(ack, Response::Ack { applied_seq: 0 }));
    }

    #[test]
    fn missing_init_is_an_error() {
        let mut output = Vec::new();
        let result = serve(
            "test-build",
            std::io::Cursor::new(Vec::new()),
            &mut output,
            fake,
        );
        assert!(result.is_err());
    }

    #[test]
    fn malformed_frame_is_an_error_not_an_exit() {
        let mut input = init_frame(80, 24);
        input.extend_from_slice(&[0, 0, 0, 0]);
        let mut output = Vec::new();
        let result = serve("test-build", std::io::Cursor::new(input), &mut output, fake);
        assert!(result.is_err());
    }

    #[test]
    fn duplicate_init_is_rejected() {
        let mut input = init_frame(80, 24);
        input.extend_from_slice(&init_frame(80, 24));
        let mut output = Vec::new();
        let result = serve("test-build", std::io::Cursor::new(input), &mut output, fake);
        assert!(result.is_err());
    }

    #[test]
    fn parent_change_is_the_only_reparent_signal() {
        assert!(!parent_changed(4814, 4814));
        assert!(parent_changed(4814, 1));
    }
}
