//! File paths on the macOS pasteboards, so the file tree interoperates with
//! Finder and other apps (Feishu, Mail, …) the way Finder's Cmd+C/Cmd+V does.
//!
//! The drag pasteboard is read too: with Tauri's native drag-drop disabled (the
//! tree and the composer rely on HTML5 drag events), a drop from Finder only
//! carries file contents to the webview, never paths. The drag pasteboard still
//! holds the dropped items' file URLs, so the drop handler reads them from here.

use serde::Serialize;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PasteboardFiles {
    /// Bumps on every pasteboard write by any app; tells "still what we
    /// copied" apart from "someone copied something else since".
    pub change_count: isize,
    pub paths: Vec<String>,
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardNameDrag, NSPasteboardTypeFileURL, NSPasteboardWriting,
    };
    use objc2_foundation::{NSArray, NSString, NSURL};

    use super::PasteboardFiles;

    pub fn write_file_paths(pasteboard: &NSPasteboard, paths: &[String]) -> Result<isize, String> {
        let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
            .iter()
            .map(|path| {
                ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(path)))
            })
            .collect();
        pasteboard.clearContents();
        if !pasteboard.writeObjects(&NSArray::from_retained_slice(&urls)) {
            return Err("the pasteboard rejected the file URLs".to_string());
        }
        Ok(pasteboard.changeCount())
    }

    pub fn read_file_paths(pasteboard: &NSPasteboard) -> PasteboardFiles {
        let paths = pasteboard
            .pasteboardItems()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let url = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
                        // Finder writes file reference URLs (file:///.file/id=…);
                        // filePathURL resolves them to a real path.
                        let path = NSURL::URLWithString(&url)?.filePathURL()?.path()?;
                        Some(path.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        PasteboardFiles {
            change_count: pasteboard.changeCount(),
            paths,
        }
    }

    pub fn general() -> Retained<NSPasteboard> {
        NSPasteboard::generalPasteboard()
    }

    pub fn drag() -> Retained<NSPasteboard> {
        NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag })
    }
}

/// Put `paths` on the general pasteboard as files; returns the new change count.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn pasteboard_write_file_paths(paths: Vec<String>) -> Result<isize, String> {
    platform::write_file_paths(&platform::general(), &paths)
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub fn pasteboard_read_file_paths() -> PasteboardFiles {
    platform::read_file_paths(&platform::general())
}

/// The file paths of the most recent drag, read while handling its drop.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn drag_pasteboard_file_paths() -> Vec<String> {
    platform::read_file_paths(&platform::drag()).paths
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn pasteboard_write_file_paths(_paths: Vec<String>) -> Result<isize, String> {
    Err("file pasteboard is only supported on macOS".to_string())
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn pasteboard_read_file_paths() -> PasteboardFiles {
    PasteboardFiles {
        change_count: 0,
        paths: Vec::new(),
    }
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn drag_pasteboard_file_paths() -> Vec<String> {
    Vec::new()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use objc2_app_kit::NSPasteboard;

    use super::platform::{read_file_paths, write_file_paths};

    #[test]
    fn written_file_paths_read_back_as_paths() {
        // A private pasteboard, so the test never touches the user's clipboard.
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let first = dir.join("报告 v2.txt");
        let second = dir.join("folder");
        std::fs::write(&first, "x").unwrap();
        std::fs::create_dir(&second).unwrap();
        let paths = vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];

        let change_count = write_file_paths(&pasteboard, &paths).unwrap();
        let read = read_file_paths(&pasteboard);

        assert_eq!(read.paths, paths);
        assert_eq!(read.change_count, change_count);
    }

    #[test]
    fn a_pasteboard_without_files_reads_as_empty() {
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();

        assert!(read_file_paths(&pasteboard).paths.is_empty());
    }
}
