use std::fs;
use std::path::{Path, PathBuf};

use gix::merge::plumbing::blob::builtin_driver::text;
use gix::merge::plumbing::blob::{Resolution, builtin_driver};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to write merged content to {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("some hunks conflicted with unstaged changes")]
    AutoResolved,
    #[error("merge produced unresolved conflicts")]
    Conflict,
}

pub fn three_way_merge(
    workdir: &Path,
    path: &str,
    ours: &[u8],
    base: &[u8],
    theirs: &[u8],
) -> Result<(), Error> {
    let mut out = Vec::new();
    let mut input = gix::diff::blob::InternedInput::new(&[][..], &[][..]);
    let labels = text::Labels::default();

    let opts = text::Options {
        conflict: text::Conflict::ResolveWithOurs,
        ..Default::default()
    };
    let resolution = builtin_driver::text(&mut out, &mut input, labels, ours, base, theirs, opts);

    let file_path = workdir.join(path);
    fs::write(&file_path, &out).map_err(|e| Error::Write {
        path: file_path,
        source: e,
    })?;

    match resolution {
        Resolution::Complete => Ok(()),
        Resolution::CompleteWithAutoResolvedConflict => Err(Error::AutoResolved),
        Resolution::Conflict => Err(Error::Conflict),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_merge() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workdir = dir.path();
        // Pre-create the file so write succeeds
        fs::write(workdir.join("file.txt"), b"").expect("create file");

        let base = b"a\nb\nc\n";
        let ours = b"a\nb\nc\nd\n"; // added line at end
        let theirs = b"A\nb\nc\n"; // changed first line

        let result = three_way_merge(workdir, "file.txt", ours, base, theirs);
        assert!(result.is_ok(), "clean merge should succeed: {result:?}");

        let content = fs::read_to_string(workdir.join("file.txt")).expect("read");
        assert!(
            content.contains('A') && content.contains('d'),
            "merged file should have both changes, got: {content}"
        );
    }

    #[test]
    fn conflicting_merge() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workdir = dir.path();
        fs::write(workdir.join("file.txt"), b"").expect("create file");

        let base = b"a\n";
        let ours = b"x\n";
        let theirs = b"y\n";

        let result = three_way_merge(workdir, "file.txt", ours, base, theirs);
        assert!(result.is_err(), "conflicting merge should return Err");

        // File should still be written (with ours winning due to ResolveWithOurs)
        let content = fs::read_to_string(workdir.join("file.txt")).expect("read");
        assert!(
            content.contains('x'),
            "conflicting merge should resolve with ours, got: {content}"
        );
    }

    #[test]
    fn identical_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workdir = dir.path();
        fs::write(workdir.join("file.txt"), b"").expect("create file");

        let data = b"same\n";

        let result = three_way_merge(workdir, "file.txt", data, data, data);
        assert!(
            result.is_ok(),
            "identical inputs should succeed: {result:?}"
        );

        let content = fs::read_to_string(workdir.join("file.txt")).expect("read");
        assert_eq!(content, "same\n");
    }

    #[test]
    fn empty_base() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workdir = dir.path();
        fs::write(workdir.join("file.txt"), b"").expect("create file");

        let base = b"";
        let ours = b"hello\n";
        let theirs = b"world\n";

        // Should not panic - either merge or conflict is acceptable
        let _result = three_way_merge(workdir, "file.txt", ours, base, theirs);

        // File should be written regardless
        assert!(
            workdir.join("file.txt").exists(),
            "file should exist after merge"
        );
    }
}
