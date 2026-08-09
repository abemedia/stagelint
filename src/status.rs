use std::collections::BTreeSet;

use gix::bstr::BString;
use gix::index::entry::{Flags, Mode};
use gix::status;
use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to compute repository status")]
    Status(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Complete working tree status collected in a single pass.
pub struct WorktreeStatus {
    /// Files with staged changes (HEAD->index additions/modifications).
    pub staged: BTreeSet<BString>,
    /// Files modified in the worktree relative to the index.
    pub dirty: BTreeSet<BString>,
    /// Untracked non-ignored files.
    pub untracked: BTreeSet<BString>,
    /// Index paths absent from the worktree (deleted or renamed away).
    pub missing: BTreeSet<BString>,
}

/// Collect working tree status: staged changes, dirty files, and optionally untracked files.
///
/// Partially-staged files appear in both `staged` and `dirty`.
pub fn collect(repo: &gix::Repository, include_untracked: bool) -> Result<WorktreeStatus, Error> {
    // The walk and rename detection serve only the untracked set. Copies stay disabled: their
    // source still exists on disk, and similarity matching could take an untracked file for one.
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
    let index = repo
        .index_or_empty()
        .map_err(|e| Error::Status(Box::new(e)))?;
    let platform = repo
        .status(gix::progress::Discard)
        .map_err(|e| Error::Status(Box::new(e)))?
        .index(index.clone().into())
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
                if index.entries()[change.index()]
                    .flags
                    .contains(Flags::SKIP_WORKTREE)
                {
                    continue;
                }
                staged.insert(change.location().to_owned());
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
                    // A removed path can't be read from disk, so track it as absent, not dirty.
                    if matches!(status, EntryStatus::Change(Change::Removed)) {
                        missing.insert(rela_path);
                    } else {
                        dirty.insert(rela_path);
                    }
                }
                status::index_worktree::Item::DirectoryContents { entry, .. } => {
                    if matches!(entry.status, gix::dir::entry::Status::Untracked)
                        && matches!(
                            entry.disk_kind,
                            Some(gix::dir::entry::Kind::File | gix::dir::entry::Kind::Symlink)
                        )
                    {
                        untracked.insert(entry.rela_path);
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
                    missing.insert(source.rela_path().into());

                    // gix consumes the destination into the Rewrite item rather than emitting it as
                    // DirectoryContents, so capture it here to hide it like any other untracked file.
                    untracked.insert(dirwalk_entry.rela_path);
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
