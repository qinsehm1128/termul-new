//! The sole process allowed to load Ghostty FFI. Stdio is a private protocol,
//! never a PTY; there are no callbacks into PTY input or host desktop services.
use base64::{engine::general_purpose::STANDARD, Engine};
use se_ghostty_vt::Terminal;
use se_screen_protocol::*;

pub fn run_with_build_id(build_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !se_ghostty_vt::NATIVE_AVAILABLE {
        return Err("native unavailable".into());
    }
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
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
    let mut terminal = Terminal::new(cols, rows).map_err(|_| "native initialization")?;
    let mut applied_seq = 0;
    write_frame(&mut output, &Response::Ack { applied_seq })?;
    loop {
        let request: Request = match read_frame(&mut input) {
            Ok(request) => request,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
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
                terminal.write(&bytes).map_err(|_| "native write")?;
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
                terminal
                    .resize(cols, rows, 0, 0)
                    .map_err(|_| "native resize")?;
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
                let screen = terminal
                    .screen_tail(max_lines, MAX_TEXT_BYTES)
                    .map_err(|_| "native screen read")?;
                let truncated = usize::from(screen.rows) > max_lines
                    || screen.text.len() >= MAX_TEXT_BYTES.saturating_sub(3);
                if screen.text.len() > MAX_TEXT_BYTES {
                    return Err("native text limit".into());
                }
                write_frame(
                    &mut output,
                    &Response::Reply {
                        request_id,
                        applied_seq,
                        text: screen.text,
                        truncated,
                    },
                )?;
            }
            Request::Init { .. } => return Err("duplicate initialization".into()),
        }
    }
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
