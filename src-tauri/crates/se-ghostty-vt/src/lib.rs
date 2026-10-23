//! Minimal Ghostty VT wrapper. **Use only inside a dedicated child process.**
//!
//! This crate does not own a PTY, emit replies, access the clipboard, or expose
//! host-effect callbacks. Native parsing is not an in-process security boundary.
//! Each terminal is thread-confined and all operations require exclusive access.

use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

pub const NATIVE_AVAILABLE: bool = cfg!(feature = "native");
pub const MAX_COLS: u16 = 512;
pub const MAX_ROWS: u16 = 256;
pub const MAX_CELLS: usize = 65_536;
pub const MAX_INPUT_BYTES: usize = 65_536;
pub const MAX_OUTPUT_BYTES: usize = 1_048_576;
pub const MAX_OUTPUT_LINES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Rebuild with `--features native` and the pinned Zig toolchain.
    Unavailable,
    InvalidDimensions,
    InputTooLarge,
    OutputLimitTooLarge,
    NativeFailure,
    InvalidUtf8,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "Ghostty VT unavailable: enable the native feature",
            Self::InvalidDimensions => "terminal dimensions exceed allowed limits",
            Self::InputTooLarge => "terminal input exceeds the per-write limit",
            Self::OutputLimitTooLarge => "snapshot limits exceed allowed limits",
            Self::NativeFailure => "Ghostty VT operation failed",
            Self::InvalidUtf8 => "Ghostty VT returned invalid UTF-8",
        })
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenSnapshot {
    pub text: String,
    pub cols: u16,
    pub rows: u16,
    pub alternate_screen: bool,
    /// Zero-based cursor coordinates.
    pub cursor_x: u16,
    pub cursor_y: u16,
}

/// Thread-confined terminal. No `Send`/`Sync`, cloning or raw handle access.
///
/// ```compile_fail
/// fn require_send<T: Send>() {}
/// require_send::<se_ghostty_vt::Terminal>();
/// ```
///
/// ```compile_fail
/// fn require_sync<T: Sync>() {}
/// require_sync::<se_ghostty_vt::Terminal>();
/// ```
pub struct Terminal {
    #[cfg(feature = "native")]
    handle: std::ptr::NonNull<std::ffi::c_void>,
    _thread_confined: PhantomData<Rc<()>>,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Result<Self, Error> {
        #[cfg(not(feature = "native"))]
        {
            let _ = (cols, rows);
            Err(Error::Unavailable)
        }
        #[cfg(feature = "native")]
        {
            dimensions(cols, rows)?;
            // SAFETY: bounded dimensions; the shim returns a uniquely owned handle.
            let handle = unsafe { ffi::se_vt_new(cols, rows) };
            Ok(Self {
                handle: std::ptr::NonNull::new(handle).ok_or(Error::NativeFailure)?,
                _thread_confined: PhantomData,
            })
        }
    }

    /// Feed one bounded chunk; parser state persists across writes.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        #[cfg(not(feature = "native"))]
        {
            let _ = bytes;
            Err(Error::Unavailable)
        }
        #[cfg(feature = "native")]
        {
            if bytes.len() > MAX_INPUT_BYTES {
                return Err(Error::InputTooLarge);
            }
            // SAFETY: exclusive live handle, valid read-only slice for the call.
            native_result(unsafe {
                ffi::se_vt_write(self.handle.as_ptr(), bytes.as_ptr(), bytes.len())
            })
        }
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), Error> {
        #[cfg(not(feature = "native"))]
        {
            let _ = (cols, rows, cell_width_px, cell_height_px);
            Err(Error::Unavailable)
        }
        #[cfg(feature = "native")]
        {
            dimensions(cols, rows)?;
            if cell_width_px > 4096 || cell_height_px > 4096 {
                return Err(Error::InvalidDimensions);
            }
            // SAFETY: exclusive handle and bounded dimensions/pixel products.
            native_result(unsafe {
                ffi::se_vt_resize(
                    self.handle.as_ptr(),
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                )
            })
        }
    }

    /// Plain text of the active screen and retained scrollback, newest lines last.
    /// Tail truncation never splits UTF-8; metadata always describes the viewport.
    pub fn screen_tail(
        &mut self,
        max_lines: usize,
        max_bytes: usize,
    ) -> Result<ScreenSnapshot, Error> {
        #[cfg(not(feature = "native"))]
        {
            let _ = (max_lines, max_bytes);
            Err(Error::Unavailable)
        }
        #[cfg(feature = "native")]
        {
            if max_lines > MAX_OUTPUT_LINES || max_bytes > MAX_OUTPUT_BYTES {
                return Err(Error::OutputLimitTooLarge);
            }
            // The shim's fixed scrollback/screen limits bound formatting. It writes
            // into this caller-owned buffer and cannot grow it behind our back.
            let mut buffer = vec![0u8; 4 * MAX_OUTPUT_BYTES];
            let mut len = 0;
            let mut meta = ffi::Meta::default();
            // SAFETY: writable buffer and metadata are valid for this exclusive call.
            native_result(unsafe {
                ffi::se_vt_snapshot(
                    self.handle.as_ptr(),
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut len,
                    &mut meta,
                )
            })?;
            if len > buffer.len() {
                return Err(Error::NativeFailure);
            }
            let text = std::str::from_utf8(&buffer[..len]).map_err(|_| Error::InvalidUtf8)?;
            Ok(ScreenSnapshot {
                text: bounded_tail(text, max_lines, max_bytes).to_owned(),
                cols: meta.cols,
                rows: meta.rows,
                alternate_screen: meta.alternate_screen != 0,
                cursor_x: meta.cursor_x,
                cursor_y: meta.cursor_y,
            })
        }
    }
}

#[cfg(feature = "native")]
impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: unique live handle, freed exactly once; no callbacks are installed.
        unsafe { ffi::se_vt_free(self.handle.as_ptr()) }
    }
}

#[cfg(feature = "native")]
fn dimensions(cols: u16, rows: u16) -> Result<(), Error> {
    if cols == 0
        || rows == 0
        || cols > MAX_COLS
        || rows > MAX_ROWS
        || usize::from(cols) * usize::from(rows) > MAX_CELLS
    {
        Err(Error::InvalidDimensions)
    } else {
        Ok(())
    }
}

#[cfg(feature = "native")]
fn native_result(result: i32) -> Result<(), Error> {
    if result == 0 {
        Ok(())
    } else {
        Err(Error::NativeFailure)
    }
}

#[cfg(any(feature = "native", test))]
fn bounded_tail(text: &str, max_lines: usize, max_bytes: usize) -> &str {
    if max_lines == 0 || max_bytes == 0 {
        return "";
    }
    let mut start = 0;
    // A final newline terminates the last line; it does not add an empty line.
    let without_final_newline = text.strip_suffix('\n').unwrap_or(text);
    for (lines, (index, _)) in without_final_newline.rmatch_indices('\n').enumerate() {
        if lines + 1 == max_lines {
            start = index + 1;
            break;
        }
    }
    start = start.max(text.len().saturating_sub(max_bytes));
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

#[cfg(feature = "native")]
mod ffi {
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    pub struct Meta {
        pub cols: u16,
        pub rows: u16,
        pub cursor_x: u16,
        pub cursor_y: u16,
        pub alternate_screen: u8,
    }
    unsafe extern "C" {
        pub fn se_vt_new(cols: u16, rows: u16) -> *mut c_void;
        pub fn se_vt_free(handle: *mut c_void);
        pub fn se_vt_write(handle: *mut c_void, data: *const u8, len: usize) -> i32;
        pub fn se_vt_resize(
            handle: *mut c_void,
            cols: u16,
            rows: u16,
            width: u32,
            height: u32,
        ) -> i32;
        pub fn se_vt_snapshot(
            handle: *mut c_void,
            data: *mut u8,
            cap: usize,
            len: *mut usize,
            meta: *mut Meta,
        ) -> i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf8_and_line_tail() {
        assert_eq!(bounded_tail("first\n中文\nlast\n", 2, 100), "中文\nlast\n");
        assert_eq!(bounded_tail("a中文", 1, 4), "文");
        assert_eq!(bounded_tail("a中文", 1, 2), "");
        assert_eq!(bounded_tail("hello", 0, 5), "");
    }
    #[cfg(not(feature = "native"))]
    #[test]
    fn unavailable_is_explicit() {
        const { assert!(!NATIVE_AVAILABLE) };
        assert!(matches!(Terminal::new(80, 24), Err(Error::Unavailable)));
        let mut terminal = Terminal {
            _thread_confined: PhantomData,
        };
        assert_eq!(terminal.write(b"hello"), Err(Error::Unavailable));
        assert_eq!(terminal.resize(80, 24, 8, 16), Err(Error::Unavailable));
        assert_eq!(terminal.screen_tail(10, 100), Err(Error::Unavailable));
    }
}
