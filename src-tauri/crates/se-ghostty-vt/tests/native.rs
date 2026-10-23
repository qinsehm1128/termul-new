#![cfg(feature = "native")]

use se_ghostty_vt::{Error, Terminal, MAX_INPUT_BYTES, MAX_OUTPUT_BYTES, NATIVE_AVAILABLE};

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(ALPHABET[a >> 2] as char);
        out.push(ALPHABET[((a & 3) << 4) | (b >> 4)] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((b & 15) << 2) | (c >> 6)] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[c & 63] as char
        } else {
            '='
        });
    }
    out
}

fn text(terminal: &mut Terminal) -> String {
    terminal.screen_tail(4096, MAX_OUTPUT_BYTES).unwrap().text
}

#[test]
fn ansi_color_is_plain_text() {
    const { assert!(NATIVE_AVAILABLE) };
    let mut vt = Terminal::new(80, 24).unwrap();
    vt.write(b"hello \x1b[31mred\x1b[0m!").unwrap();
    let s = vt.screen_tail(24, 4096).unwrap();
    assert_eq!(s.text.trim_end(), "hello red!");
    assert!(!s.text.contains('\x1b'));
    assert_eq!((s.cursor_x, s.cursor_y), (10, 0));
}

#[test]
fn carriage_return_and_cursor_overwrite() {
    let mut vt = Terminal::new(40, 10).unwrap();
    vt.write(b"abcdef\rXY\x1b[1;5HZ").unwrap();
    assert_eq!(text(&mut vt).trim_end(), "XYcdZf");
}

#[test]
fn alternate_screen_round_trip() {
    let mut vt = Terminal::new(40, 10).unwrap();
    vt.write(b"primary\x1b[?1049halternate").unwrap();
    let s = vt.screen_tail(10, 4096).unwrap();
    assert!(s.alternate_screen);
    assert!(s.text.contains("alternate"));
    assert!(!s.text.contains("primary"));
    vt.write(b"\x1b[?1049l").unwrap();
    let s = vt.screen_tail(10, 4096).unwrap();
    assert!(!s.alternate_screen);
    assert_eq!(s.text.trim_end(), "primary");
}

#[test]
fn resize_preserves_text_and_reports_dimensions() {
    let mut vt = Terminal::new(80, 24).unwrap();
    vt.write(b"resize me").unwrap();
    vt.resize(40, 12, 8, 16).unwrap();
    let s = vt.screen_tail(12, 4096).unwrap();
    assert_eq!((s.cols, s.rows), (40, 12));
    assert!(s.text.contains("resize me"));
    assert!(s.cursor_x < s.cols && s.cursor_y < s.rows);
}

#[test]
fn split_ansi_and_split_chinese_utf8() {
    let mut vt = Terminal::new(80, 24).unwrap();
    for chunk in [b"\x1b[".as_slice(), b"32", b"m"] {
        vt.write(chunk).unwrap();
    }
    for byte in "中文测试".as_bytes() {
        vt.write(&[*byte]).unwrap();
    }
    vt.write(b"\x1b[0m").unwrap();
    assert_eq!(text(&mut vt).trim_end(), "中文测试");
}

#[test]
fn snapshot_tail_and_zero_limits() {
    let mut vt = Terminal::new(40, 4).unwrap();
    vt.write("first\r\nsecond\r\n中文\r\nlast".as_bytes())
        .unwrap();
    let s = vt.screen_tail(2, 1024).unwrap();
    assert_eq!(s.text.trim_end(), "中文\nlast");
    let s = vt.screen_tail(100, 7).unwrap();
    assert!(s.text.len() <= 7);
    assert!(s.text.ends_with("last"));
    assert!(vt.screen_tail(0, 10).unwrap().text.is_empty());
    assert!(vt.screen_tail(10, 0).unwrap().text.is_empty());
    assert_eq!(
        vt.screen_tail(1, MAX_OUTPUT_BYTES + 1),
        Err(Error::OutputLimitTooLarge)
    );
}

#[test]
fn scrollback_tail_is_bounded() {
    let mut vt = Terminal::new(40, 3).unwrap();
    for i in 0..1000 {
        vt.write(format!("line-{i}\r\n").as_bytes()).unwrap();
    }
    let full = text(&mut vt);
    assert!(!full.contains("line-0\n"));
    assert!(
        full.lines().count() <= 3,
        "zero scrollback must return only the viewport: {full:?}"
    );
    let s = vt.screen_tail(2, 100).unwrap();
    assert!(s.text.contains("line-999"));
    assert!(s.text.len() <= 100);
    assert!(s.text.lines().count() <= 2);
}

#[test]
fn hostile_queries_and_host_effects_are_inert() {
    let mut vt = Terminal::new(80, 24).unwrap();
    vt.write(b"before\x1b[5n\x1b[6n\x1b[c\x1b[>c\x1b]52;c;c2VjcmV0\x07\x07\x1b]0;host-title\x07")
        .unwrap();
    // Kitty file, temporary file, shared memory and direct-image requests. No
    // host callback is installed, all image media and image storage are disabled.
    let path = std::env::temp_dir().join(format!(
        "tty-graphics-protocol-se-vt-{}",
        std::process::id()
    ));
    std::fs::write(&path, [0u8; 4]).unwrap();
    let encoded_path = base64(path.to_str().unwrap().as_bytes());
    for medium in ['f', 't'] {
        vt.write(format!("\x1b_Ga=T,t={medium},f=32,s=1,v=1;{encoded_path}\x1b\\").as_bytes())
            .unwrap();
    }
    vt.write(b"\x1b_Ga=T,t=s,f=100;L3NlLWdob3N0dHktdnQ=\x1b\\\x1b_Ga=T,t=d,f=32,s=1,v=1;AAAAAA==\x1b\\after").unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        [0u8; 4],
        "Kitty temp-file input must not unlink or modify files"
    );
    std::fs::remove_file(path).unwrap();
    assert_eq!(text(&mut vt).trim_end(), "beforeafter");
    // Overlong APC input is consumed with a fixed accumulator budget.
    vt.write(b"\x1b_G").unwrap();
    for _ in 0..8 {
        vt.write(&vec![b'x'; MAX_INPUT_BYTES]).unwrap();
    }
    vt.write(b"\x1b\\ok").unwrap();
    assert!(text(&mut vt).contains("ok"));
}

#[test]
fn resource_limits_are_enforced() {
    assert!(matches!(
        Terminal::new(0, 24),
        Err(Error::InvalidDimensions)
    ));
    assert!(matches!(
        Terminal::new(1025, 24),
        Err(Error::InvalidDimensions)
    ));
    assert!(matches!(
        Terminal::new(1024, 257),
        Err(Error::InvalidDimensions)
    ));
    let mut vt = Terminal::new(80, 24).unwrap();
    assert_eq!(
        vt.write(&vec![b'x'; MAX_INPUT_BYTES + 1]),
        Err(Error::InputTooLarge)
    );
    assert_eq!(vt.resize(80, 0, 0, 0), Err(Error::InvalidDimensions));
    assert_eq!(
        vt.resize(80, 24, u32::MAX, 16),
        Err(Error::InvalidDimensions)
    );
    assert_eq!(vt.screen_tail(4097, 10), Err(Error::OutputLimitTooLarge));
}
