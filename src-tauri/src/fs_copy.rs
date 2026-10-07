//! Copying entries into a directory without ever overwriting, Finder-style.
//!
//! Paste and drop must not destroy data: a destination name that is already
//! taken gets a "name copy" sibling instead of being replaced. That also covers
//! pasting a file back into its own folder, where `std::fs::copy(a, a)`
//! truncates `a` to zero bytes.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Copies each source (file, directory tree, or symlink) into `target_dir`
/// and returns the created paths in source order.
pub fn copy_entries_into(sources: &[PathBuf], target_dir: &Path) -> io::Result<Vec<PathBuf>> {
    if !fs::metadata(target_dir)?.is_dir() {
        return Err(invalid_input(format!(
            "{} is not a directory",
            target_dir.display()
        )));
    }
    let canonical_target = fs::canonicalize(target_dir)?;
    // Validate every source before copying any, so one bad entry does not
    // leave a half-finished paste behind.
    for source in sources {
        if fs::symlink_metadata(source)?.is_dir()
            && canonical_target.starts_with(fs::canonicalize(source)?)
        {
            return Err(invalid_input(format!(
                "cannot copy {} into itself",
                source.display()
            )));
        }
    }

    sources
        .iter()
        .map(|source| {
            let name = source
                .file_name()
                .ok_or_else(|| invalid_input(format!("{} has no name", source.display())))?;
            let is_dir = fs::symlink_metadata(source)?.is_dir();
            let destination = available_destination(target_dir, name, is_dir);
            copy_entry(source, &destination)?;
            Ok(destination)
        })
        .collect()
}

/// `dir/name` when free, otherwise the first free "name copy", "name copy 2", …
/// A file keeps its extension last ("a copy.txt"); a folder name is never split.
fn available_destination(dir: &Path, name: &OsStr, is_dir: bool) -> PathBuf {
    let candidate = dir.join(name);
    if !exists(&candidate) {
        return candidate;
    }
    let name = name.to_string_lossy();
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 && !is_dir => name.split_at(dot),
        _ => (name.as_ref(), ""),
    };
    (1..)
        .map(|n| {
            let suffix = if n == 1 {
                " copy".to_string()
            } else {
                format!(" copy {n}")
            };
            dir.join(format!("{stem}{suffix}{extension}"))
        })
        .find(|candidate| !exists(candidate))
        .expect("an unbounded counter always finds a free name")
}

/// Exists as anything, including a dangling symlink.
fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn copy_entry(source: &Path, destination: &Path) -> io::Result<()> {
    let file_type = fs::symlink_metadata(source)?.file_type();
    if file_type.is_symlink() {
        copy_symlink(source, destination)
    } else if file_type.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(source, destination).map(|_| ())
    }
}

#[cfg(unix)]
fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)
}

#[cfg(windows)]
fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    let target = fs::read_link(source)?;
    if fs::metadata(source).is_ok_and(|meta| meta.is_dir()) {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

fn invalid_input(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Desktop paste/drop: copy `sources` into `target_dir`, never overwriting.
#[tauri::command]
pub async fn fs_copy_entries(
    sources: Vec<String>,
    target_dir: String,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sources: Vec<PathBuf> = sources.into_iter().map(PathBuf::from).collect();
        copy_entries_into(&sources, Path::new(&target_dir))
    })
    .await
    .map_err(|e| format!("copy task failed: {e}"))?
    .map(|created| {
        created
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn pasting_a_file_into_its_own_folder_keeps_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        fs::write(&file, "payload").unwrap();

        let created = copy_entries_into(std::slice::from_ref(&file), dir.path()).unwrap();

        assert_eq!(created, vec![dir.path().join("notes copy.txt")]);
        assert_eq!(read(&file), "payload");
        assert_eq!(read(&created[0]), "payload");
    }

    #[test]
    fn a_taken_name_is_never_overwritten() {
        let source_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("a.txt");
        fs::write(&source, "new").unwrap();
        fs::write(target.path().join("a.txt"), "old").unwrap();
        fs::write(target.path().join("a copy.txt"), "old copy").unwrap();

        let created = copy_entries_into(&[source], target.path()).unwrap();

        assert_eq!(created, vec![target.path().join("a copy 2.txt")]);
        assert_eq!(read(&target.path().join("a.txt")), "old");
        assert_eq!(read(&target.path().join("a copy.txt")), "old copy");
        assert_eq!(read(&created[0]), "new");
    }

    #[test]
    fn names_without_an_extension_and_folders_are_not_split() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".env"), "x").unwrap();
        fs::create_dir(dir.path().join("lib.d")).unwrap();

        let created = copy_entries_into(
            &[dir.path().join(".env"), dir.path().join("lib.d")],
            dir.path(),
        )
        .unwrap();

        assert_eq!(
            created,
            vec![dir.path().join(".env copy"), dir.path().join("lib.d copy")]
        );
    }

    #[test]
    fn a_folder_is_copied_with_its_whole_tree() {
        let source_dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let folder = source_dir.path().join("src");
        fs::create_dir_all(folder.join("nested/deeper")).unwrap();
        fs::write(folder.join("top.rs"), "top").unwrap();
        fs::write(folder.join("nested/deeper/leaf.rs"), "leaf").unwrap();

        let created = copy_entries_into(&[folder], target.path()).unwrap();

        assert_eq!(created, vec![target.path().join("src")]);
        assert_eq!(read(&target.path().join("src/top.rs")), "top");
        assert_eq!(
            read(&target.path().join("src/nested/deeper/leaf.rs")),
            "leaf"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_copied_as_links() {
        let dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let folder = dir.path().join("pkg");
        fs::create_dir(&folder).unwrap();
        std::os::unix::fs::symlink("../elsewhere", folder.join("link")).unwrap();

        copy_entries_into(&[folder], target.path()).unwrap();

        assert_eq!(
            fs::read_link(target.path().join("pkg/link")).unwrap(),
            PathBuf::from("../elsewhere")
        );
    }

    #[test]
    fn a_folder_cannot_be_copied_into_itself_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("src");
        fs::create_dir_all(folder.join("inner")).unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "x").unwrap();

        let error = copy_entries_into(&[file, folder.clone()], &folder.join("inner")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!folder.join("inner/a.txt").exists());
        assert_eq!(fs::read_dir(folder.join("inner")).unwrap().count(), 0);
    }

    #[test]
    fn duplicating_a_folder_into_its_parent_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("src");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("main.rs"), "fn main() {}").unwrap();

        let created = copy_entries_into(&[folder], dir.path()).unwrap();

        assert_eq!(created, vec![dir.path().join("src copy")]);
        assert_eq!(read(&dir.path().join("src copy/main.rs")), "fn main() {}");
    }

    #[test]
    fn the_target_must_be_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "x").unwrap();

        assert!(copy_entries_into(std::slice::from_ref(&file), &file).is_err());
    }
}
