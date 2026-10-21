//! Terminal modes in force at the start of a terminal's retained output.
//!
//! A full-screen program turns on mouse reporting, bracketed paste or the
//! alternate screen once, when it starts. The output log keeps only the newest
//! bytes, so after long enough those switches are evicted. A renderer that
//! replays the log from the start then shows the program without its modes:
//! clicks and the wheel never reach it, and it looks frozen.
//!
//! [`ModeTracker`] follows the evicted bytes, so it always knows the modes at
//! the oldest retained chunk, and [`ModeTracker::preamble`] turns them back on
//! ahead of a replay.

/// DEC private modes a program sets once and a replay must restore.
/// Alternate screen first: switching buffers must not undo the others.
const TRACKED_MODES: &[u16] = &[
    1049, 1047, 47, // alternate screen
    1,  // application cursor keys
    25, // cursor visible (on by default)
    1000, 1002, 1003, // mouse reporting
    1005, 1006, 1015, // mouse encodings
    1004, // focus events
    2004, // bracketed paste
];

/// Longest partial escape sequence carried to the next chunk. Anything longer
/// is not a mode switch.
const MAX_CARRY: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct ModeTracker {
    /// Explicitly set state of each tracked mode; absent means default.
    modes: Vec<(u16, bool)>,
    /// Application keypad (`ESC =`) versus numeric (`ESC >`).
    application_keypad: bool,
    /// A sequence split across chunks.
    carry: Vec<u8>,
}

fn default_state(mode: u16) -> bool {
    mode == 25
}

impl ModeTracker {
    /// Follow `data`, the next bytes of output.
    pub fn feed(&mut self, data: &[u8]) {
        let mut bytes = std::mem::take(&mut self.carry);
        bytes.extend_from_slice(data);
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != 0x1b {
                index += 1;
                continue;
            }
            match self.parse_escape(&bytes[index..]) {
                Parsed::Complete(length) => index += length,
                Parsed::Incomplete => {
                    let rest = &bytes[index..];
                    if rest.len() <= MAX_CARRY {
                        self.carry = rest.to_vec();
                    }
                    return;
                }
            }
        }
    }

    /// Bytes that turn on every mode not in its default state.
    #[must_use]
    pub fn preamble(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for &mode in TRACKED_MODES {
            let Some(&(_, on)) = self.modes.iter().find(|(tracked, _)| *tracked == mode) else {
                continue;
            };
            if on != default_state(mode) {
                out.extend_from_slice(
                    format!("\x1b[?{mode}{}", if on { 'h' } else { 'l' }).as_bytes(),
                );
            }
        }
        if self.application_keypad {
            out.extend_from_slice(b"\x1b=");
        }
        out
    }

    fn set(&mut self, mode: u16, on: bool) {
        if !TRACKED_MODES.contains(&mode) {
            return;
        }
        match self.modes.iter_mut().find(|(tracked, _)| *tracked == mode) {
            Some(entry) => entry.1 = on,
            None => self.modes.push((mode, on)),
        }
    }

    /// `bytes` starts with ESC.
    fn parse_escape(&mut self, bytes: &[u8]) -> Parsed {
        match bytes.get(1) {
            None => Parsed::Incomplete,
            Some(b'=') => {
                self.application_keypad = true;
                Parsed::Complete(2)
            }
            Some(b'>') => {
                self.application_keypad = false;
                Parsed::Complete(2)
            }
            Some(b'c') => {
                // RIS: full reset.
                self.modes.clear();
                self.application_keypad = false;
                Parsed::Complete(2)
            }
            Some(b'[') => self.parse_csi(bytes),
            Some(_) => Parsed::Complete(1),
        }
    }

    fn parse_csi(&mut self, bytes: &[u8]) -> Parsed {
        let private = bytes.get(2) == Some(&b'?');
        let start = if private { 3 } else { 2 };
        let mut end = start;
        while let Some(&byte) = bytes.get(end) {
            if (0x40..=0x7e).contains(&byte) {
                if private && (byte == b'h' || byte == b'l') {
                    let on = byte == b'h';
                    let params = &bytes[start..end];
                    for param in params.split(|&byte| byte == b';') {
                        if let Some(mode) = std::str::from_utf8(param)
                            .ok()
                            .and_then(|text| text.parse::<u16>().ok())
                        {
                            self.set(mode, on);
                        }
                    }
                }
                return Parsed::Complete(end + 1);
            }
            if !(0x20..=0x3f).contains(&byte) {
                // Not a CSI after all; resume at the next byte.
                return Parsed::Complete(1);
            }
            end += 1;
        }
        Parsed::Incomplete
    }
}

enum Parsed {
    Complete(usize),
    Incomplete,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preamble_after(chunks: &[&[u8]]) -> String {
        let mut tracker = ModeTracker::default();
        for chunk in chunks {
            tracker.feed(chunk);
        }
        String::from_utf8(tracker.preamble()).unwrap()
    }

    #[test]
    fn restores_the_modes_a_tui_turned_on_at_startup() {
        // crossterm's EnableMouseCapture, bracketed paste, focus and the
        // alternate screen, then ordinary output.
        let startup = b"\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h\x1b[?2004h\x1b[?1004h\x1b[?25lhello";
        assert_eq!(
            preamble_after(&[startup]),
            "\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1015h\x1b[?1004h\x1b[?2004h"
        );
    }

    #[test]
    fn a_mode_turned_off_again_is_not_restored() {
        assert_eq!(
            preamble_after(&[b"\x1b[?1000h\x1b[?2004h", b"\x1b[?1000l"]),
            "\x1b[?2004h"
        );
        assert_eq!(preamble_after(&[b"\x1b[?25l", b"\x1b[?25h"]), "");
    }

    #[test]
    fn follows_a_sequence_split_across_chunks_and_combined_parameters() {
        assert_eq!(
            preamble_after(&[b"text\x1b[?10", b"06;2004h"]),
            "\x1b[?1006h\x1b[?2004h"
        );
        assert_eq!(preamble_after(&[b"\x1b", b"[?1000h"]), "\x1b[?1000h");
    }

    #[test]
    fn ignores_other_sequences_and_resets_on_ris() {
        assert_eq!(
            preamble_after(&[b"\x1b[31m\x1b[2J\x1b[?9999h\x1b[1000h"]),
            ""
        );
        assert_eq!(preamble_after(&[b"\x1b=", b"\x1b[?1000h\x1bc"]), "");
        assert_eq!(preamble_after(&[b"\x1b="]), "\x1b=");
    }
}
