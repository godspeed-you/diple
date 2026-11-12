//! The file behind a document, and whether it has changed since it was read.
//!
//! # How a change is noticed
//!
//! By asking the filesystem, not by subscribing to it. `stat(2)` on one path
//! every poll interval costs nothing measurable, needs no dependency and no
//! platform API, and — unlike a watch on an inode — keeps working when an
//! editor saves by writing a new file and renaming it over the old one, which
//! is what vim, most IDEs and every "atomic save" do. The path is what the
//! reader opened, so the path is what is watched.
//!
//! # Why a change has to settle first
//!
//! A file is not written in one instant. An editor may truncate it and then
//! write it in pieces, or remove it a moment before the rename lands. Reading
//! at the first sign of a change would sometimes show half a document, or a
//! JSON file that does not parse yet. So a change is only reported once the
//! file has looked the same on two consecutive checks; the caller polls every
//! quarter of a second, which puts a reload a quarter to half a second after
//! the save — immediate to a reader, and long enough for the write to finish.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What `stat(2)` says about the file: enough to tell two versions apart.
///
/// The modification time alone is not: two saves within the filesystem's
/// timestamp granularity look the same, and a rename keeps the time of the
/// new file. Size, inode and change time together catch what the modification
/// time misses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    #[cfg(unix)]
    inode: (u64, u64),
    #[cfg(unix)]
    changed: (i64, i64),
}

impl Stamp {
    /// The current stamp of `path`, or `None` when it cannot be read — gone,
    /// or between the unlink and the rename of an atomic save.
    fn of(path: &Path) -> Option<Stamp> {
        let meta = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(Stamp {
            modified: meta.modified().ok(),
            len: meta.len(),
            #[cfg(unix)]
            inode: (meta.dev(), meta.ino()),
            #[cfg(unix)]
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }
}

/// What a check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// The file is as it was when it was read.
    None,
    /// It differs, but it was still changing at the last check; ask again.
    Settling,
    /// It differs and has stopped changing: read it again.
    Changed,
    /// It is gone, and stayed gone. Reported once; it is watched for its
    /// return.
    Vanished,
}

/// A document's file: its path and the version that was last read.
#[derive(Debug, Clone)]
pub(crate) struct SourceFile {
    path: PathBuf,
    /// The stamp of the version on screen (`None` once the file vanished).
    loaded: Option<Stamp>,
    /// A stamp that differed from `loaded` at the last check, and so has to
    /// be seen once more before it counts.
    pending: Option<Option<Stamp>>,
}

impl SourceFile {
    /// The file at `path`, stamped now — that is, *before* it is read, so a
    /// write that races the read is seen as a change rather than lost.
    pub(crate) fn new(path: impl Into<PathBuf>) -> SourceFile {
        let path = path.into();
        let loaded = Stamp::of(&path);
        SourceFile {
            path,
            loaded,
            pending: None,
        }
    }

    /// The path as it was given.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Read the file again, as text.
    ///
    /// Stamps first, for the same reason [`SourceFile::new`] does. Invalid
    /// UTF-8 is decoded lossily, as it is at startup: a reader is better
    /// served by a document with a replacement character than by none.
    pub(crate) fn read(&mut self) -> io::Result<String> {
        self.loaded = Stamp::of(&self.path);
        self.pending = None;
        let bytes = std::fs::read(&self.path)?;
        Ok(match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
        })
    }

    /// Whether the file changed since it was last read.
    ///
    /// Every answer but [`Change::Settling`] is final for that version: a
    /// change that was reported is not reported again until the file changes
    /// once more, whether or not the caller managed to read it.
    pub(crate) fn check(&mut self) -> Change {
        let now = Stamp::of(&self.path);
        if now == self.loaded {
            self.pending = None;
            return Change::None;
        }
        if self.pending != Some(now) {
            self.pending = Some(now);
            return Change::Settling;
        }
        self.pending = None;
        self.loaded = now;
        if now.is_some() {
            Change::Changed
        } else {
            Change::Vanished
        }
    }
}

/// Whether two paths name the same file, so a reload of one can be passed on
/// to every view of it.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(name);
        (dir, path)
    }

    #[test]
    fn an_untouched_file_reports_nothing() {
        let (_dir, path) = temp("a.md");
        fs::write(&path, "# A\n").expect("write");
        let mut file = SourceFile::new(&path);
        assert_eq!(file.read().expect("read"), "# A\n");
        for _ in 0..3 {
            assert_eq!(file.check(), Change::None);
        }
    }

    #[test]
    fn a_change_is_reported_once_it_has_settled_and_only_once() {
        let (_dir, path) = temp("a.md");
        fs::write(&path, "# A\n").expect("write");
        let mut file = SourceFile::new(&path);
        file.read().expect("read");

        fs::write(&path, "# A\n\nmore\n").expect("write");
        assert_eq!(file.check(), Change::Settling, "first sight");
        assert_eq!(file.check(), Change::Changed, "the same on the next look");
        assert_eq!(file.check(), Change::None, "not reported twice");
    }

    #[test]
    fn a_file_still_being_written_keeps_settling() {
        let (_dir, path) = temp("a.md");
        fs::write(&path, "x").expect("write");
        let mut file = SourceFile::new(&path);
        for text in ["xy", "xyz", "xyzw"] {
            fs::write(&path, text).expect("write");
            assert_eq!(file.check(), Change::Settling, "{text}");
        }
        assert_eq!(file.check(), Change::Changed);
    }

    #[test]
    fn an_atomic_save_is_a_change() {
        let (dir, path) = temp("a.md");
        fs::write(&path, "same length").expect("write");
        let mut file = SourceFile::new(&path);
        let staged = dir.path().join("a.md.tmp");
        fs::write(&staged, "same LENGTH").expect("write");
        fs::rename(&staged, &path).expect("rename");
        assert_eq!(file.check(), Change::Settling);
        assert_eq!(file.check(), Change::Changed);
        assert_eq!(file.read().expect("read"), "same LENGTH");
    }

    #[test]
    fn a_vanished_file_is_reported_once_and_its_return_is_a_change() {
        let (_dir, path) = temp("a.md");
        fs::write(&path, "one").expect("write");
        let mut file = SourceFile::new(&path);
        fs::remove_file(&path).expect("remove");
        assert_eq!(file.check(), Change::Settling);
        assert_eq!(file.check(), Change::Vanished);
        assert_eq!(file.check(), Change::None, "gone is the new normal");

        fs::write(&path, "two").expect("write");
        assert_eq!(file.check(), Change::Settling);
        assert_eq!(file.check(), Change::Changed);
    }

    #[test]
    fn a_brief_absence_during_a_save_is_not_a_vanishing() {
        let (_dir, path) = temp("a.md");
        fs::write(&path, "one").expect("write");
        let mut file = SourceFile::new(&path);
        fs::remove_file(&path).expect("remove");
        assert_eq!(file.check(), Change::Settling);
        fs::write(&path, "two").expect("write");
        assert_eq!(
            file.check(),
            Change::Settling,
            "a different stamp starts over"
        );
        assert_eq!(file.check(), Change::Changed);
    }

    #[test]
    fn two_spellings_of_one_path_are_the_same_file() {
        let (dir, path) = temp("a.md");
        fs::write(&path, "x").expect("write");
        let dotted = dir.path().join(".").join("a.md");
        assert!(same_file(&path, &dotted));
        assert!(!same_file(&path, &dir.path().join("b.md")));
    }
}
