//! Terminal input goes through one writer thread per terminal.
//!
//! A write to a PTY blocks once the program on the other side stops reading
//! stdin and the kernel input queue fills (about 1 KiB in raw mode on macOS).
//! Written inline, that stall held the terminal's writer lock, which cleanup
//! takes first, and the core's request loop behind it, so one program that
//! stopped reading froze input, resize and close for every terminal. Queued,
//! the stall stays on this thread until the program reads again or the PTY
//! goes away.

use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

/// Input a program may leave unread before further writes are refused.
pub const MAX_PENDING_INPUT: usize = 4 * 1024 * 1024;

pub struct QueuedInput {
    tx: mpsc::Sender<Vec<u8>>,
    pending: Arc<AtomicUsize>,
}

impl QueuedInput {
    pub fn spawn(terminal_id: &str, mut writer: Box<dyn Write + Send>) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let pending = Arc::new(AtomicUsize::new(0));
        let drained = Arc::clone(&pending);
        let id = terminal_id.to_string();
        std::thread::Builder::new()
            .name(format!("pty-input-{terminal_id}"))
            .spawn(move || {
                // Ends when the terminal drops its input and the queue is
                // drained, or when the PTY refuses a write.
                for chunk in rx {
                    let written = writer.write_all(&chunk).and_then(|()| writer.flush());
                    drained.fetch_sub(chunk.len(), Ordering::AcqRel);
                    if let Err(error) = written {
                        log::info!("[PTY {id}] input writer stopped: {error}");
                        break;
                    }
                }
            })?;
        Ok(Self { tx, pending })
    }
}

impl Write for QueuedInput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let queued = self.pending.fetch_add(buf.len(), Ordering::AcqRel);
        if queued + buf.len() > MAX_PENDING_INPUT {
            self.pending.fetch_sub(buf.len(), Ordering::AcqRel);
            return Err(io::Error::other("terminal is not reading its input"));
        }
        if self.tx.send(buf.to_vec()).is_err() {
            self.pending.fetch_sub(buf.len(), Ordering::AcqRel);
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal input is closed",
            ));
        }
        Ok(buf.len())
    }

    /// The writer thread flushes after every chunk.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A sink that records what reached the PTY side, optionally held shut.
    #[derive(Clone, Default)]
    struct Sink {
        written: Arc<Mutex<Vec<u8>>>,
        gate: Arc<Mutex<()>>,
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let _open = self.gate.lock().unwrap();
            self.written.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn wait_for(sink: &Sink, expected: &[u8]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while sink.written.lock().unwrap().as_slice() != expected {
            assert!(
                std::time::Instant::now() < deadline,
                "input reached the PTY"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn input_reaches_the_pty_in_order() {
        let sink = Sink::default();
        let mut input = QueuedInput::spawn("t", Box::new(sink.clone())).unwrap();
        input.write_all(b"ab").unwrap();
        input.write_all(b"cd").unwrap();
        wait_for(&sink, b"abcd");
    }

    #[test]
    fn a_backlog_past_the_limit_is_refused_until_it_drains() {
        let sink = Sink::default();
        let held = sink.gate.lock().unwrap();
        let mut input = QueuedInput::spawn("t", Box::new(sink.clone())).unwrap();

        input.write_all(&vec![b'x'; MAX_PENDING_INPUT]).unwrap();
        assert!(input.write_all(b"y").is_err(), "the backlog is full");

        drop(held);
        wait_for(&sink, &vec![b'x'; MAX_PENDING_INPUT]);
        input.write_all(b"y").unwrap();
    }
}
