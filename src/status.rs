use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use gix::bstr::BString;
use gix::diff::index::Change;
use gix::index::entry::{Flags, Mode};
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
    #[error("bare repository")]
    BareRepo,
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
}

impl Source<'_> {
    /// Whether the run stages what the commands wrote, which is what needs the stash.
    pub fn stages_results(self) -> bool {
        matches!(self, Source::Staged | Source::Diff(_))
    }
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
    walk_untracked: bool,
    source: Source<'_>,
) -> Result<Status, Error> {
    // Named paths are the scope, so no status pass is needed.
    if let Source::Files(paths) = source {
        let workdir = repo.workdir().ok_or(Error::BareRepo)?;
        return Ok(Status {
            scope: file_scope(workdir, paths)?,
            ..Status::default()
        });
    }
    let stashing = source.stages_results();
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
    let iter = repo
        .status(gix::progress::Discard)
        .map_err(|e| Error::Status(Box::new(e)))?
        .index(index.clone().into())
        .index_worktree_rewrites(rewrites)
        .untracked_files(untracked_files)
        .tree_index_track_renames(status::tree_index::TrackRenames::Disabled)
        .index_worktree_submodules(None)
        .into_iter(Vec::<BString>::new())
        .map_err(|e| Error::Status(Box::new(e)))?;

    let mut result = Status {
        scope: match source {
            Source::Diff(spec) => diff_scope(repo, spec, &index)?,
            _ => BTreeSet::new(),
        },
        ..Status::default()
    };
    for item in iter {
        match item.map_err(|e| Error::Status(Box::new(e)))? {
            status::Item::TreeIndex(change) => {
                // Only a run that stashes reads what `tree_index` records.
                if stashing {
                    tree_index(change, source, &index, &mut result);
                }
            }
            status::Item::IndexWorktree(item) => index_worktree(item, source, &mut result),
        }
    }
    Ok(result)
}

/// Repo-relative paths for the regular files among `paths` inside the worktree; anything else is
/// skipped rather than rejected.
fn file_scope(workdir: &Path, paths: &[PathBuf]) -> Result<BTreeSet<BString>, Error> {
    let cwd = std::env::current_dir().map_err(Error::Workdir)?;
    let workdir = workdir.canonicalize().map_err(Error::Workdir)?;
    Ok(paths
        .iter()
        .filter_map(|path| {
            let path = if path.is_absolute() {
                Cow::Borrowed(path.as_path())
            } else {
                Cow::Owned(cwd.join(path))
            };
            // Symlinked parents have to be resolved for the prefix to match, but the final
            // component is left alone so that a symlinked file is still skipped.
            if !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file()) {
                return None;
            }
            let resolved = path.parent()?.canonicalize().ok()?.join(path.file_name()?);
            let rela_path = resolved.strip_prefix(&workdir).ok()?;
            // Everything else in `scope` comes from git, which is always slash-separated.
            let rela_path = gix::path::into_bstr(rela_path);
            Some(gix::path::to_unix_separators_on_windows(rela_path).into_owned())
        })
        .collect())
}

/// Regular files changed in `spec` that exist on disk, resolved as `git diff <spec>` would.
fn diff_scope(
    repo: &gix::Repository,
    spec: &str,
    index: &gix::index::State,
) -> Result<BTreeSet<BString>, Error> {
    let tree = |id: gix::ObjectId| {
        repo.find_object(id)
            .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?
            .peel_to_tree()
            .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))
    };
    let (from, to) = match repo
        .rev_parse(spec)
        .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?
        .detach()
    {
        gix::revision::plumbing::Spec::Range { from, to } => (tree(from)?, tree(to)?),
        gix::revision::plumbing::Spec::Merge { theirs, ours } => {
            let base = repo
                .merge_base(theirs, ours)
                .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?;
            (tree(base.detach())?, tree(ours)?)
        }
        gix::revision::plumbing::Spec::Include(from) => (
            tree(from)?,
            repo.head_tree()
                .map_err(|e| Error::Revspec(spec.to_owned(), Box::new(e)))?,
        ),
        _ => return Err(Error::NotARange(spec.to_owned())),
    };
    let workdir = repo.workdir().ok_or(Error::BareRepo)?;

    let mut scope = BTreeSet::new();
    from.changes()
        .map_err(|e| Error::Status(Box::new(e)))?
        .options(|opts| {
            opts.track_rewrites(None);
        })
        .for_each_to_obtain_tree(&to, |change| {
            if !matches!(change, gix::object::tree::diff::Change::Deletion { .. })
                && change.entry_mode().is_blob()
                // No index entry means nothing to stage the result into.
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

/// Record a HEAD->index change, scoping it when the run covers staged files.
fn tree_index(
    change: Change,
    source: Source<'_>,
    index: &gix::worktree::Index,
    result: &mut Status,
) {
    if matches!(source, Source::Staged)
        && !matches!(change, Change::Deletion { .. })
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
fn index_worktree(item: status::index_worktree::Item, source: Source<'_>, result: &mut Status) {
    let unstaged = matches!(source, Source::Unstaged);
    let stashing = source.stages_results();
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
            } else if unstaged {
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
            } else if unstaged && matches!(entry.disk_kind, Some(gix::dir::entry::Kind::File)) {
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
