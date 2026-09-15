use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gix::bstr::BString;
use gix::diff::index::Change;
use gix::index::entry::{Flags, Mode};
use gix::repository::normalize_path;
use gix::status;
use gix::status::plumbing::index_as_worktree::{self, EntryStatus};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to compute repository status")]
    Status(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("failed to resolve revision {0}")]
    Revspec(String, #[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("{0} is not a diff range")]
    NotARange(String),
    #[error("failed to resolve the working directory")]
    Workdir(#[source] std::io::Error),
    #[error("failed to resolve path {}", .0.display())]
    Path(PathBuf, #[source] normalize_path::Error),
}

/// Where the file set comes from.
#[derive(Clone, Copy)]
pub enum Source<'a> {
    /// Files staged for commit.
    Staged,
    /// Files changed in a revision range, resolved as `git diff <spec>` would.
    Diff(&'a str),
    /// Files modified in the worktree but not staged, including untracked ones.
    Unstaged,
    /// Paths named on the command line.
    Files(&'a [PathBuf]),
    /// Every file in the working tree that is not ignored.
    All,
}

/// Everything one status pass knows about the repository.
#[derive(Default)]
pub struct Status {
    /// HEAD->index changes, unfiltered.
    pub changes: Vec<Change>,
    /// Files the commands receive: no symlinks, submodules or deletions.
    pub scope: BTreeSet<BString>,
    /// Files modified in the worktree relative to the index.
    pub dirty: BTreeSet<BString>,
    /// Untracked non-ignored files.
    pub untracked: BTreeSet<BString>,
    /// Index paths absent from the worktree (deleted or renamed away).
    pub missing: BTreeSet<BString>,
}

/// Collect repository status, walking untracked files when asked or when the source needs them.
///
/// The scope is whatever `source` selects. Partially-staged files appear in both `changes` and
/// `dirty`.
pub fn collect(
    repo: &gix::Repository,
    workdir: &Path,
    walk_untracked: bool,
    source: Source<'_>,
) -> Result<Status, Error> {
    // These sources find their files without a status pass, and nothing reads the rest of it.
    let scope = match source {
        Source::Files(paths) => Some(file_scope(repo, paths)?),
        Source::Diff(spec) => Some(diff_scope(repo, workdir, spec)?),
        Source::All => Some(all_scope(repo, workdir)?),
        Source::Staged | Source::Unstaged => None,
    };
    if let Some(scope) = scope {
        return Ok(Status {
            scope,
            ..Status::default()
        });
    }
    let stashing = matches!(source, Source::Staged);
    // Untracked files are part of the unstaged scope, whatever `--stash` asked for.
    let untracked_files = if walk_untracked || matches!(source, Source::Unstaged) {
        status::UntrackedFiles::Files
    } else {
        status::UntrackedFiles::None
    };
    // Rename detection serves only the stash's untracked bookkeeping. Copies stay disabled even
    // then: their source still exists on disk, and matching could take an untracked file for one.
    let rewrites = (walk_untracked && stashing).then(|| gix::diff::Rewrites {
        copies: None,
        ..Default::default()
    });
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

    let mut result = Status::default();
    // Only a run that stashes reads what `tree_index` records.
    if stashing {
        let iter = platform
            .into_iter(Vec::<BString>::new())
            .map_err(|e| Error::Status(Box::new(e)))?;
        for item in iter {
            match item.map_err(|e| Error::Status(Box::new(e)))? {
                status::Item::TreeIndex(change) => tree_index(change, &index, &mut result),
                status::Item::IndexWorktree(item) => index_worktree(item, stashing, &mut result),
            }
        }
    } else {
        let iter = platform
            .into_index_worktree_iter(Vec::<BString>::new())
            .map_err(|e| Error::Status(Box::new(e)))?;
        for item in iter {
            let item = item.map_err(|e| Error::Status(Box::new(e)))?;
            index_worktree(item, stashing, &mut result);
        }
    }
    Ok(result)
}

/// Repo-relative paths for `paths` inside the worktree, whether or not they exist.
fn file_scope(repo: &gix::Repository, paths: &[PathBuf]) -> Result<BTreeSet<BString>, Error> {
    let cwd = std::env::current_dir().map_err(Error::Workdir)?;
    let mut scope = BTreeSet::new();
    for path in paths {
        // Make paths absolute and drop `..` first: `normalize_path` reads relative paths from the
        // root inside `.git`, and rejects `../repo/x` even though it ends up inside.
        let Some(absolute) = gix::path::normalize(Cow::Owned(cwd.join(path)), &cwd) else {
            continue;
        };
        match repo.normalize_path(&gix::path::into_bstr(absolute)) {
            Ok(rela_path) => scope.insert(rela_path.into_owned()),
            Err(
                normalize_path::Error::OutsideOfRepository { .. }
                | normalize_path::Error::AbsolutePathOutsideOfRepository { .. },
            ) => continue,
            Err(e) => return Err(Error::Path(path.clone(), e)),
        };
    }
    Ok(scope)
}

/// Regular files on disk that are tracked, skipping those git keeps out of the working tree, or
/// untracked and not ignored.
fn all_scope(repo: &gix::Repository, workdir: &Path) -> Result<BTreeSet<BString>, Error> {
    let index = repo
        .index_or_empty()
        .map_err(|e| Error::Status(Box::new(e)))?;
    let options = repo
        .dirwalk_options()
        .map_err(|e| Error::Status(Box::new(e)))?
        .emit_untracked(gix::dir::walk::EmissionMode::Matching);
    let untracked = repo
        .dirwalk_iter(
            index.clone(),
            Vec::<BString>::new(),
            Arc::<AtomicBool>::default().into(),
            options,
        )
        .map_err(|e| Error::Status(Box::new(e)))?;

    let mut scope: BTreeSet<BString> = index
        .entries()
        .iter()
        .filter(|entry| eligible(entry.mode, entry.flags))
        .map(|entry| entry.path(&index))
        .filter(|path| {
            std::fs::symlink_metadata(workdir.join(gix::path::from_bstr(*path)))
                .is_ok_and(|meta| meta.is_file())
        })
        .map(ToOwned::to_owned)
        .collect();
    for item in untracked {
        let entry = item.map_err(|e| Error::Status(Box::new(e)))?.entry;
        if matches!(entry.status, gix::dir::entry::Status::Untracked)
            && matches!(entry.disk_kind, Some(gix::dir::entry::Kind::File))
        {
            scope.insert(entry.rela_path);
        }
    }
    Ok(scope)
}

/// Regular files changed in `spec` that exist on disk, resolved as `git diff <spec>` would.
fn diff_scope(
    repo: &gix::Repository,
    workdir: &Path,
    spec: &str,
) -> Result<BTreeSet<BString>, Error> {
    let (from, to) = match repo
        .rev_parse(spec)
        .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?
        .detach()
    {
        gix::revision::plumbing::Spec::Range { from, to } => (from, to),
        gix::revision::plumbing::Spec::Merge { theirs, ours } => {
            let base = repo
                .merge_base(theirs, ours)
                .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?;
            (base.detach(), ours)
        }
        gix::revision::plumbing::Spec::Include(from) => {
            let head = repo
                .head_id()
                .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?;
            (from, head.detach())
        }
        _ => return Err(Error::NotARange(spec.to_owned())),
    };
    let from = repo
        .find_object(from)
        .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?
        .peel_to_tree()
        .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?;
    let to = repo
        .find_object(to)
        .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?
        .peel_to_tree()
        .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?;
    let index = repo
        .index_or_empty()
        .map_err(|e| Error::Status(Box::new(e)))?;

    let mut scope = BTreeSet::new();
    from.changes()
        .map_err(|e| Error::Status(Box::new(e)))?
        .options(|opts| {
            opts.track_rewrites(None);
        })
        .for_each_to_obtain_tree(&to, |change| {
            if !matches!(change, gix::object::tree::diff::Change::Deletion { .. })
                && change.entry_mode().is_blob()
                // A path no longer tracked may now be ignored, so it is left alone.
                && index
                    .entry_by_path(change.location())
                    .is_some_and(|entry| !entry.flags.contains(Flags::SKIP_WORKTREE))
                && std::fs::symlink_metadata(workdir.join(gix::path::from_bstr(change.location())))
                    .is_ok_and(|meta| meta.is_file())
            {
                scope.insert(change.location().to_owned());
            }
            Ok::<_, std::convert::Infallible>(gix::object::tree::diff::Action::Continue(()))
        })
        .map_err(|e| Error::Status(Box::new(e)))?;
    Ok(scope)
}

/// Record a HEAD->index change, scoping it when a command can be given the file.
fn tree_index(change: Change, index: &gix::worktree::Index, result: &mut Status) {
    if !matches!(change, Change::Deletion { .. })
        && eligible(change.entry_mode(), index.entries()[change.index()].flags)
    {
        result.scope.insert(change.location().to_owned());
    }
    result.changes.push(change);
}

/// Sort an index-worktree difference into the dirty, untracked and missing sets, scoping it when
/// the run covers unstaged files.
///
/// Only a run that stashes reads those sets.
fn index_worktree(item: status::index_worktree::Item, stashing: bool, result: &mut Status) {
    match item {
        status::index_worktree::Item::Modification {
            entry,
            rela_path,
            status: state,
            ..
        } => {
            // Submodules have no file content to stash or restore.
            if entry.mode == Mode::COMMIT {
                return;
            }
            // A removed path can't be read from disk, so track it as absent, not dirty.
            if matches!(
                state,
                EntryStatus::Change(index_as_worktree::Change::Removed)
            ) {
                if stashing {
                    result.missing.insert(rela_path);
                }
            } else if stashing {
                result.dirty.insert(rela_path);
            } else {
                // A type change leaves the index mode stale; the worktree is what a command opens.
                let mode = match &state {
                    EntryStatus::Change(index_as_worktree::Change::Type { worktree_mode }) => {
                        *worktree_mode
                    }
                    _ => entry.mode,
                };
                if eligible(mode, entry.flags) {
                    result.scope.insert(rela_path);
                }
            }
        }
        status::index_worktree::Item::DirectoryContents { entry, .. } => {
            if !matches!(entry.status, gix::dir::entry::Status::Untracked) {
                return;
            }
            if stashing {
                if matches!(
                    entry.disk_kind,
                    Some(gix::dir::entry::Kind::File | gix::dir::entry::Kind::Symlink)
                ) {
                    result.untracked.insert(entry.rela_path);
                }
            } else if matches!(entry.disk_kind, Some(gix::dir::entry::Kind::File)) {
                result.scope.insert(entry.rela_path);
            }
        }
        status::index_worktree::Item::Rewrite {
            source: from,
            dirwalk_entry,
            ..
        } => {
            // Copies are disabled, so a Rewrite is always a rename. Only a stashing run enables
            // rename detection, so `--unstaged` sees the destination as an untracked file instead.
            if stashing {
                // Source path is gone from the worktree. Record it so the run receives indexed
                // content and the restore step deletes it afterward.
                result.missing.insert(from.rela_path().into());
                // gix consumes the destination into the Rewrite item rather than emitting it as
                // DirectoryContents, so capture it here to hide it like any other untracked file.
                result.untracked.insert(dirwalk_entry.rela_path);
            }
        }
    }
}

/// Whether an entry can be given to a command: a regular file git keeps in the working tree.
fn eligible(mode: Mode, flags: Flags) -> bool {
    matches!(mode, Mode::FILE | Mode::FILE_EXECUTABLE) && !flags.contains(Flags::SKIP_WORKTREE)
}
