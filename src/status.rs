use std::collections::BTreeSet;
use std::path::Path;

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
}

/// Everything one status pass knows about the repository.
pub struct Status {
    /// HEAD->index changes, unfiltered.
    pub changes: Vec<Change>,
    /// Files the run lints: no symlinks, submodules, deletions or skip-worktree entries.
    pub scope: BTreeSet<BString>,
    /// Files modified in the worktree relative to the index.
    pub dirty: BTreeSet<BString>,
    /// Untracked non-ignored files.
    pub untracked: BTreeSet<BString>,
    /// Index paths absent from the worktree (deleted or renamed away).
    pub missing: BTreeSet<BString>,
}

/// Collect repository status, optionally including untracked files.
///
/// The scope is the staged files, or the files changed in `diff` when given.
/// Partially-staged files appear in both `changes` and `dirty`.
pub fn collect(
    repo: &gix::Repository,
    include_untracked: bool,
    diff: Option<&str>,
) -> Result<Status, Error> {
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

    let mut changes = Vec::new();
    let mut scope = match diff {
        Some(spec) => diff_scope(repo, spec, &index)?,
        None => BTreeSet::new(),
    };
    let mut dirty = BTreeSet::new();
    let mut untracked = BTreeSet::new();
    let mut missing = BTreeSet::new();

    for item in iter {
        match item.map_err(|e| Error::Status(Box::new(e)))? {
            status::Item::TreeIndex(change) => {
                if diff.is_none()
                    && !matches!(change.entry_mode(), Mode::SYMLINK | Mode::COMMIT)
                    && !matches!(change, Change::Deletion { .. })
                    && !index.entries()[change.index()]
                        .flags
                        .contains(Flags::SKIP_WORKTREE)
                {
                    scope.insert(change.location().to_owned());
                }
                changes.push(change);
            }
            status::Item::IndexWorktree(item) => match item {
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
                    if matches!(
                        status,
                        EntryStatus::Change(index_as_worktree::Change::Removed)
                    ) {
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

    Ok(Status {
        changes,
        scope,
        dirty,
        untracked,
        missing,
    })
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
    let workdir = repo.workdir().unwrap_or(Path::new(""));

    let mut scope = BTreeSet::new();
    from.changes()
        .map_err(|e| Error::Status(Box::new(e)))?
        .options(|opts| {
            opts.track_rewrites(None);
        })
        .for_each_to_obtain_tree(&to, |change| {
            if !matches!(change, gix::object::tree::diff::Change::Deletion { .. })
                && change.entry_mode().is_blob()
                && !index
                    .entry_by_path(change.location())
                    .is_some_and(|entry| entry.flags.contains(Flags::SKIP_WORKTREE))
                && workdir
                    .join(gix::path::from_bstr(change.location()))
                    .is_file()
            {
                scope.insert(change.location().to_owned());
            }
            Ok::<_, std::convert::Infallible>(gix::object::tree::diff::Action::Continue(()))
        })
        .map_err(|e| Error::Status(Box::new(e)))?;
    Ok(scope)
}
