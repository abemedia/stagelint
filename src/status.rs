use std::collections::BTreeSet;

use gix::bstr::ByteSlice;
use gix::index::entry::Mode;
use gix::status;
use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("non-UTF-8 path in index")]
    NonUtf8Path,
    #[error("failed to compute repository status")]
    Status(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Complete working tree status collected in a single pass.
pub struct WorktreeStatus {
    /// Files with staged changes (HEAD->index additions/modifications).
    pub staged: BTreeSet<String>,
    /// Files modified in the worktree relative to the index.
    pub dirty: BTreeSet<String>,
    /// Untracked non-ignored files.
    pub untracked: BTreeSet<String>,
    /// Index paths absent from the worktree (deleted or renamed away).
    pub missing: BTreeSet<String>,
}

/// Collect working tree status: staged changes, dirty files, and optionally untracked files.
///
/// Partially-staged files appear in both `staged` and `dirty`.
pub fn collect(repo: &gix::Repository, include_untracked: bool) -> Result<WorktreeStatus, Error> {
    // The directory walk and rename detection only serve the untracked set: a rename destination
    // must be found on disk to be hidden, and its source is reported as Rewrite rather than
    // silently missing. Copies are explicitly disabled: a copy source still exists on disk, so
    // there is nothing to hide or restore, and content-similarity matching could misclassify a new
    // untracked file as a copy of a tracked one. Without the walk, a rename source still surfaces
    // as Removed and feeds `missing`.
    let (rewrites, untracked_files) = if include_untracked {
        (
            Some(gix::diff::Rewrites {
                copies: None,
                ..Default::default()
            }),
            status::UntrackedFiles::Files,
        )
    } else {
        (None, status::UntrackedFiles::None)
    };
    let platform = repo
        .status(gix::progress::Discard)
        .map_err(|e| Error::Status(Box::new(e)))?
        .index_worktree_rewrites(rewrites)
        .untracked_files(untracked_files)
        .tree_index_track_renames(status::tree_index::TrackRenames::Disabled)
        .index_worktree_submodules(None);

    let iter = platform
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(|e| Error::Status(Box::new(e)))?;

    let mut staged = BTreeSet::new();
    let mut dirty = BTreeSet::new();
    let mut untracked = BTreeSet::new();
    let mut missing = BTreeSet::new();

    for item in iter {
        let item = item.map_err(|e| Error::Status(Box::new(e)))?;
        match item {
            status::Item::TreeIndex(change) => {
                if matches!(change.entry_mode(), Mode::SYMLINK | Mode::COMMIT) {
                    continue;
                }
                if let gix::diff::index::Change::Deletion { .. } = &change {
                    continue;
                }
                let path = change.location().to_str().map_err(|_| Error::NonUtf8Path)?;
                staged.insert(path.to_owned());
            }
            status::Item::IndexWorktree(iw_item) => match iw_item {
                status::index_worktree::Item::Modification {
                    entry,
                    rela_path,
                    status,
                    ..
                } => {
                    // Submodules have no file content to stash or restore.
                    if entry.mode == Mode::COMMIT {
                        continue;
                    }
                    let path = rela_path.to_str().map_err(|_| Error::NonUtf8Path)?;
                    // A removed path can't be read from disk, so track it as absent, not dirty.
                    if matches!(status, EntryStatus::Change(Change::Removed)) {
                        missing.insert(path.to_owned());
                    } else {
                        dirty.insert(path.to_owned());
                    }
                }
                status::index_worktree::Item::DirectoryContents { entry, .. } => {
                    if include_untracked
                        && matches!(entry.status, gix::dir::entry::Status::Untracked)
                        && matches!(
                            entry.disk_kind,
                            Some(gix::dir::entry::Kind::File | gix::dir::entry::Kind::Symlink)
                        )
                    {
                        let path = entry.rela_path.to_str().map_err(|_| Error::NonUtf8Path)?;
                        untracked.insert(path.to_owned());
                    }
                }
                status::index_worktree::Item::Rewrite {
                    source,
                    dirwalk_entry,
                    ..
                } => {
                    // Copies are disabled, so a Rewrite is always a rename.
                    // Source path is gone from the worktree. Record it so the run receives indexed
                    // content and the restore step deletes it afterward.
                    let src = source
                        .rela_path()
                        .to_str()
                        .map_err(|_| Error::NonUtf8Path)?;
                    missing.insert(src.to_owned());

                    // gix consumes the destination into the Rewrite item rather than emitting it as
                    // DirectoryContents, so capture it here when include_untracked is set (i.e.
                    // --stash untracked/all) so it gets hidden during the run like any other
                    // untracked file.
                    if include_untracked {
                        let dst = dirwalk_entry
                            .rela_path
                            .to_str()
                            .map_err(|_| Error::NonUtf8Path)?;
                        untracked.insert(dst.to_owned());
                    }
                }
            },
        }
    }

    Ok(WorktreeStatus {
        staged,
        dirty,
        untracked,
        missing,
    })
}
