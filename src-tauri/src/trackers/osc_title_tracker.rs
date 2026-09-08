//! OSC 0/2 window title capture from PTY output.
//!
//! Agent CLIs write their live status into the terminal title — `claude` emits a
//! spinner glyph while a turn is running and a different glyph once it settles —
//! so the title is the cheapest reliable answer to "is this agent working, or is
//! it stuck waiting for me". Raw output activity cannot answer that: an agent
//! blocked on a permission prompt and an agent that finished both produce zero
//! bytes.
//!
//! Unlike `ExitCodeTracker`, this parses raw bytes instead of a per-chunk
//! `String::from_utf8_lossy`, for two reasons:
//!
//! - A title sequence can straddle a PTY read boundary, so a per-chunk pattern
//!   match would miss it entirely.
//! - Titles carry non-ASCII status glyphs, and lossy-decoding a chunk that ends
//!   mid-codepoint corrupts exactly the character worth reading.
//!
//! One tracker is owned by one PTY reader thread, so the parse state needs no
//! lock. Retained state for pull-style reads lives in `TerminalEventHub`'s
//! snapshot, alongside cwd and exit code.

use super::{TerminalEvent, TerminalEventHub};

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Hard cap on one OSC body. A child that opens a sequence and never terminates
/// it must not grow this buffer without bound; past the cap the body is dropped
/// and the rest of that sequence is discarded.
const MAX_BODY_BYTES: usize = 4096;

/// Cap on the retained title. Title text is untrusted child output, and for an
/// agent it is model-influenced, so bound what is stored, logged and forwarded.
const MAX_TITLE_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum OscScanState {
    #[default]
    Ground,
    /// Saw `ESC`, waiting to learn which sequence this is.
    Escape,
    /// Inside an OSC body, accumulating payload bytes.
    Body,
    /// Saw `ESC` inside an OSC body — `\` makes it the ST terminator.
    BodyEscape,
    /// Inside a DCS/APC/PM/SOS string whose payload may contain `]`.
    IgnoringString,
    IgnoringStringEscape,
    /// The body outgrew `MAX_BODY_BYTES`; drop bytes until the sequence ends.
    Discarding,
    DiscardingEscape,
}

/// `ESC P` (DCS), `ESC _` (APC), `ESC ^` (PM) and `ESC X` (SOS) all introduce a
/// string whose payload can legally contain `]`. Tracking them separately stops
/// such a payload from being mistaken for the start of an OSC sequence.
fn is_string_intro(byte: u8) -> bool {
    matches!(byte, b'P' | b'_' | b'^' | b'X')
}

/// Incrementally scans PTY output for OSC 0/2 titles and emits
/// `TerminalEvent::OscTitleChanged` whenever the title actually changes.
pub struct OscTitleTracker {
    terminal_id: String,
    events: TerminalEventHub,
    state: OscScanState,
    body: Vec<u8>,
    last_title: Option<String>,
}

impl OscTitleTracker {
    pub fn new(terminal_id: String, events: TerminalEventHub) -> Self {
        Self {
            terminal_id,
            events,
            state: OscScanState::default(),
            body: Vec::new(),
            last_title: None,
        }
    }

    /// Feeds one raw PTY chunk. Parse state carries across calls, so a sequence
    /// split over several reads is still recognized.
    pub fn observe(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match self.state {
                OscScanState::Ground => {
                    if byte == ESC {
                        self.state = OscScanState::Escape;
                    }
                }
                OscScanState::Escape => match byte {
                    b']' => {
                        self.body.clear();
                        self.state = OscScanState::Body;
                    }
                    // A second ESC re-arms the introducer rather than aborting.
                    ESC => {}
                    byte if is_string_intro(byte) => self.state = OscScanState::IgnoringString,
                    _ => self.state = OscScanState::Ground,
                },
                OscScanState::Body => match byte {
                    BEL => self.finish_body(),
                    ESC => self.state = OscScanState::BodyEscape,
                    _ => self.push_body(byte),
                },
                // `push_body` can overflow into `Discarding`, so every arm here
                // re-checks the state before writing more payload.
                OscScanState::BodyEscape => match byte {
                    b'\\' => self.finish_body(),
                    ESC => {
                        self.push_body(ESC);
                        if self.state == OscScanState::Body {
                            self.state = OscScanState::BodyEscape;
                        }
                    }
                    BEL => {
                        self.push_body(ESC);
                        if self.state == OscScanState::Body {
                            self.finish_body();
                        } else {
                            self.state = OscScanState::Ground;
                        }
                    }
                    // A stray ESC inside the body is malformed. Keep it as
                    // payload and stay in the body so the sequence still ends at
                    // its terminator instead of swallowing the rest of the stream.
                    _ => {
                        self.push_body(ESC);
                        if self.state == OscScanState::Body {
                            self.push_body(byte);
                        }
                    }
                },
                // Only the terminator matters for an ignored string. BEL is
                // accepted alongside ST even though it is not standard here:
                // ending an ignored string early costs nothing (its payload is
                // discarded either way), while missing its terminator would
                // blind this terminal to every later title.
                OscScanState::IgnoringString => match byte {
                    BEL => self.state = OscScanState::Ground,
                    ESC => self.state = OscScanState::IgnoringStringEscape,
                    _ => {}
                },
                OscScanState::IgnoringStringEscape => {
                    if byte == b'\\' {
                        self.state = OscScanState::Ground;
                    } else if byte != ESC {
                        self.state = OscScanState::IgnoringString;
                    }
                }
                OscScanState::Discarding => match byte {
                    BEL => self.state = OscScanState::Ground,
                    ESC => self.state = OscScanState::DiscardingEscape,
                    _ => {}
                },
                OscScanState::DiscardingEscape => {
                    if byte == b'\\' {
                        self.state = OscScanState::Ground;
                    } else if byte != ESC {
                        self.state = OscScanState::Discarding;
                    }
                }
            }
        }
    }

    /// The title retained for this terminal, or `None` if none has been seen or
    /// the child cleared it.
    ///
    /// The reader thread emits every change through the event hub rather than
    /// polling, so this exists for the tests that assert what a byte sequence
    /// left behind.
    #[cfg(test)]
    pub fn current_title(&self) -> Option<&str> {
        self.last_title.as_deref()
    }

    fn push_body(&mut self, byte: u8) {
        self.body.push(byte);
        if self.body.len() > MAX_BODY_BYTES {
            self.body.clear();
            self.state = OscScanState::Discarding;
        } else {
            self.state = OscScanState::Body;
        }
    }

    fn finish_body(&mut self) {
        self.state = OscScanState::Ground;
        // Taken so the parsed payload can be read while `self` is mutably
        // borrowed to emit; the allocation is handed back for reuse.
        let body = std::mem::take(&mut self.body);
        if let Some(payload) = window_title_payload(&body) {
            let title = sanitize_title(payload);
            let title = (!title.is_empty()).then_some(title);
            if self.last_title != title {
                self.last_title.clone_from(&title);
                self.events.emit(TerminalEvent::OscTitleChanged {
                    terminal_id: self.terminal_id.clone(),
                    title,
                });
            }
        }
        self.body = body;
        self.body.clear();
    }
}

/// Returns the payload of an OSC body when it sets the window title.
///
/// OSC 0 sets icon name and window title, OSC 2 sets the window title. OSC 1
/// sets only the icon name and is ignored, as are all other OSC commands.
fn window_title_payload(body: &[u8]) -> Option<&[u8]> {
    let separator = body.iter().position(|&byte| byte == b';')?;
    match &body[..separator] {
        b"0" | b"2" => Some(&body[separator + 1..]),
        _ => None,
    }
}

/// Decodes the payload and drops control characters, which a title cannot carry
/// and which would corrupt any surface that renders or logs it.
fn sanitize_title(payload: &[u8]) -> String {
    String::from_utf8_lossy(payload)
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_TITLE_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::TryRecvError;

    /// Builds a tracker plus a receiver for the events it emits.
    fn tracker() -> (
        OscTitleTracker,
        tokio::sync::broadcast::Receiver<TerminalEvent>,
    ) {
        let events = TerminalEventHub::standalone();
        let rx = events.subscribe();
        (OscTitleTracker::new("term-1".to_string(), events), rx)
    }

    fn next_title(rx: &mut tokio::sync::broadcast::Receiver<TerminalEvent>) -> Option<String> {
        match rx.try_recv() {
            Ok(TerminalEvent::OscTitleChanged { terminal_id, title }) => {
                assert_eq!(terminal_id, "term-1");
                title
            }
            Ok(other) => panic!("unexpected event: {other:?}"),
            Err(error) => panic!("expected a title event, got {error:?}"),
        }
    }

    fn assert_no_event(rx: &mut tokio::sync::broadcast::Receiver<TerminalEvent>) {
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "expected no title event"
        );
    }

    #[test]
    fn captures_osc_0_title_terminated_by_bel() {
        let (mut t, mut rx) = tracker();
        t.observe(b"\x1b]0;claude\x07");
        assert_eq!(next_title(&mut rx).as_deref(), Some("claude"));
        assert_eq!(t.current_title(), Some("claude"));
    }

    #[test]
    fn captures_osc_2_title_terminated_by_st() {
        let (mut t, mut rx) = tracker();
        t.observe(b"\x1b]2;codex\x1b\\");
        assert_eq!(next_title(&mut rx).as_deref(), Some("codex"));
    }

    #[test]
    fn ignores_osc_1_icon_name_and_other_commands() {
        let (mut t, mut rx) = tracker();
        t.observe(b"\x1b]1;icon\x07\x1b]7;file:///tmp\x07\x1b]133;D;0\x07");
        assert_no_event(&mut rx);
        assert_eq!(t.current_title(), None);
    }

    #[test]
    fn reassembles_a_title_split_across_chunks() {
        let (mut t, mut rx) = tracker();
        // The split lands inside the payload, which is where a per-chunk match
        // would lose the sequence.
        t.observe(b"\x1b]0;cla");
        assert_no_event(&mut rx);
        t.observe(b"ude\x07");
        assert_eq!(next_title(&mut rx).as_deref(), Some("claude"));
    }

    #[test]
    fn reassembles_a_title_split_inside_the_introducer() {
        let (mut t, mut rx) = tracker();
        t.observe(b"\x1b");
        t.observe(b"]0;pi\x07");
        assert_eq!(next_title(&mut rx).as_deref(), Some("pi"));
    }

    #[test]
    fn reassembles_a_multibyte_glyph_split_across_chunks() {
        let (mut t, mut rx) = tracker();
        // "✳ claude" — the spinner glyph is the whole point of reading the
        // title, and it is split mid-codepoint here. Byte-level accumulation
        // must recover it; a per-chunk lossy decode would emit U+FFFD.
        let title = "\u{2733} claude";
        let bytes = format!("\x1b]0;{title}\x07").into_bytes();
        let (head, tail) = bytes.split_at(6);
        t.observe(head);
        t.observe(tail);
        assert_eq!(next_title(&mut rx).as_deref(), Some(title));
    }

    #[test]
    fn emits_only_when_the_title_changes() {
        let (mut t, mut rx) = tracker();
        t.observe(b"\x1b]0;claude\x07");
        assert_eq!(next_title(&mut rx).as_deref(), Some("claude"));
        t.observe(b"\x1b]0;claude\x07");
        assert_no_event(&mut rx);
        t.observe(b"\x1b]0;claude \xe2\x9c\xb3\x07");
        assert_eq!(next_title(&mut rx).as_deref(), Some("claude ✳"));
    }

    #[test]
    fn empty_payload_clears_the_title() {
        let (mut t, mut rx) = tracker();
        t.observe(b"\x1b]0;claude\x07");
        assert_eq!(next_title(&mut rx).as_deref(), Some("claude"));
        t.observe(b"\x1b]0;\x07");
        assert_eq!(next_title(&mut rx), None);
        assert_eq!(t.current_title(), None);
        // Already cleared: clearing again is not a change.
        t.observe(b"\x1b]2;\x07");
        assert_no_event(&mut rx);
    }

    #[test]
    fn strips_control_characters_from_the_payload() {
        let (mut t, mut rx) = tracker();
        t.observe(b"\x1b]0;cla\x01ude\x1b[31m\x07");
        assert_eq!(next_title(&mut rx).as_deref(), Some("claude[31m"));
    }

    #[test]
    fn caps_an_oversized_title() {
        let (mut t, mut rx) = tracker();
        let long = "a".repeat(MAX_TITLE_CHARS + 50);
        t.observe(format!("\x1b]0;{long}\x07").as_bytes());
        assert_eq!(
            next_title(&mut rx).map(|title| title.chars().count()),
            Some(MAX_TITLE_CHARS)
        );
    }

    #[test]
    fn drops_a_body_that_never_terminates_and_recovers_afterwards() {
        let (mut t, mut rx) = tracker();
        t.observe(b"\x1b]0;");
        t.observe(&vec![b'a'; MAX_BODY_BYTES + 1]);
        assert_no_event(&mut rx);
        // The overflowed sequence is discarded up to its terminator, and the
        // next well-formed title is still captured.
        t.observe(b"\x07\x1b]0;claude\x07");
        assert_eq!(next_title(&mut rx).as_deref(), Some("claude"));
    }

    #[test]
    fn does_not_read_a_title_out_of_a_dcs_payload() {
        let (mut t, mut rx) = tracker();
        // A tmux passthrough carrying what looks like an OSC title must not be
        // mistaken for one.
        t.observe(b"\x1bPtmux;\x1b]0;inner\x07\x1b\\");
        assert_no_event(&mut rx);
        t.observe(b"\x1b]0;outer\x07");
        assert_eq!(next_title(&mut rx).as_deref(), Some("outer"));
    }

    #[test]
    fn ignores_plain_output_without_sequences() {
        let (mut t, mut rx) = tracker();
        t.observe(b"just some normal terminal output\r\n$ ");
        assert_no_event(&mut rx);
        assert_eq!(t.current_title(), None);
    }
}
