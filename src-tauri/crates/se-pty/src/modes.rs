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

const MOUSE_PROTOCOLS: &[u16] = &[1000, 1002, 1003];

/// Longest partial escape sequence carried to the next chunk. Anything longer
/// is not a mode switch.
const MAX_CARRY: usize = 64;

/// Snapshot of the modes that decide how mouse and keyboard input is routed.
/// Carries mode numbers only, never terminal content.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputModes {
    /// Active mouse protocol (1000, 1002 or 1003); `None` = no mouse reporting.
    pub mouse_tracking: Option<u16>,
    /// Active mouse encoding (1006 SGR, 1015 urxvt, 1005 UTF-8).
    pub mouse_encoding: Option<u16>,
    pub bracketed_paste: bool,
    pub application_cursor: bool,
    pub focus_events: bool,
    pub application_keypad: bool,
    pub alternate_screen: bool,
}

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

    /// Input-routing modes currently in force, for a renderer that must agree
    /// with the program about who owns the mouse and keyboard encodings.
    #[must_use]
    pub fn input_modes(&self) -> InputModes {
        let on = |mode: u16| {
            self.modes
                .iter()
                .find(|(tracked, _)| *tracked == mode)
                .map_or(default_state(mode), |(_, on)| *on)
        };
        InputModes {
            mouse_tracking: MOUSE_PROTOCOLS.iter().copied().find(|&mode| on(mode)),
            mouse_encoding: [1006, 1015, 1005].into_iter().find(|&mode| on(mode)),
            bracketed_paste: on(2004),
            application_cursor: on(1),
            focus_events: on(1004),
            application_keypad: self.application_keypad,
            alternate_screen: on(1049) || on(1047) || on(47),
        }
    }

    fn set(&mut self, mode: u16, on: bool) {
        if MOUSE_PROTOCOLS.contains(&mode) {
            // xterm keeps one active mouse protocol: each SET replaces the
            // previous protocol, while any supported protocol RESET disables
            // mouse reporting altogether.
            self.modes
                .retain(|(tracked, _)| !MOUSE_PROTOCOLS.contains(tracked));
            if on {
                self.modes.push((mode, true));
            }
            return;
        }
        if mode == 9 && !on {
            // xterm's legacy mode 9 reset also resets the active mouse
            // protocol, although mode 9 itself is not tracked/restored.
            self.modes
                .retain(|(tracked, _)| !MOUSE_PROTOCOLS.contains(tracked));
            return;
        }
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
            "\x1b[?1049h\x1b[?25l\x1b[?1003h\x1b[?1006h\x1b[?1015h\x1b[?1004h\x1b[?2004h"
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
    fn mouse_protocol_set_is_mutually_exclusive_and_any_reset_clears_it() {
        // xterm's 1000/1002/1003 modes share one active protocol; the last SET
        // wins, and resetting any supported protocol disables mouse reporting.
        assert_eq!(preamble_after(&[b"\x1b[?1002h\x1b[?1000h"]), "\x1b[?1000h");
        assert_eq!(preamble_after(&[b"\x1b[?1002h\x1b[?1000h\x1b[?1002l"]), "");
    }

    #[test]
    fn mouse_protocol_handles_reverse_order_combined_and_split_csi() {
        assert_eq!(
            preamble_after(&[b"\x1b[?1003h\x1b[?1002h\x1b[?1000h"]),
            "\x1b[?1000h"
        );
        assert_eq!(preamble_after(&[b"\x1b[?1000;1002h"]), "\x1b[?1002h");
        assert_eq!(preamble_after(&[b"\x1b[?1003", b"h"]), "\x1b[?1003h");
    }

    #[test]
    fn mouse_reset_and_ris_do_not_change_non_mouse_modes() {
        assert_eq!(
            preamble_after(&[b"\x1b[?1006h\x1b[?2004h\x1b[?1003h\x1b[?9l"]),
            "\x1b[?1006h\x1b[?2004h"
        );
        assert_eq!(preamble_after(&[b"\x1b[?1006h\x1b[?1003h\x1bc"]), "");
        assert_eq!(
            preamble_after(&[b"\x1b[?1003h\x1b[?1000l\x1b[?1002h"]),
            "\x1b[?1002h"
        );
    }

    fn input_modes_after(chunks: &[&[u8]]) -> InputModes {
        let mut tracker = ModeTracker::default();
        for chunk in chunks {
            tracker.feed(chunk);
        }
        tracker.input_modes()
    }

    #[test]
    fn input_modes_report_crossterm_capture_and_follow_resets() {
        let startup = b"\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h\x1b[?2004h\x1b[?1004h";
        assert_eq!(
            input_modes_after(&[startup]),
            InputModes {
                mouse_tracking: Some(1003),
                mouse_encoding: Some(1006),
                bracketed_paste: true,
                application_cursor: false,
                focus_events: true,
                application_keypad: false,
                alternate_screen: true,
            }
        );
        // A cross-number reset clears the protocol, as xterm does.
        let after_reset = input_modes_after(&[b"\x1b[?1002h", b"\x1b[?1000l"]);
        assert_eq!(after_reset.mouse_tracking, None);
        assert_eq!(input_modes_after(&[]), InputModes::default());
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
