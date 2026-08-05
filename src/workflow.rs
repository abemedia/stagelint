// The gix errors in `Error` push `Result` sizes past the lint threshold; a CLI never feels it.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use gix::filter::plumbing::driver::apply::Delay;
use gix::filter::plumbing::pipeline::convert::to_worktree;
use gix::index::entry::{self, Stage};
use gix::index::{fs::Metadata, write};
use gix::lock::acquire::Fail;
use gix::merge::blob::builtin_driver::text::Labels;
use gix::merge::blob::platform::builtin_merge::Pick;
use gix::merge::blob::{Resolution, ResourceKind, pipeline::WorktreeRoots};
use gix::objs::Commit;
use gix::objs::tree::{EntryKind, EntryMode};
use gix::refs::Target;
use gix::refs::log::Line;
use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
use gix::status::plumbing::index_as_worktree::EntryStatus;
use gix::{Id, ObjectId, Repository};

use crate::status::WorktreeStatus;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read tree from commit")]
    TreeDecode(#[source] gix::object::commit::Error),
    #[error("failed to create tree editor")]
    TreeEditInit(#[source] gix::object::tree::editor::init::Error),
    #[error("failed to edit tree")]
    TreeEdit(#[source] gix::objs::tree::editor::Error),
    #[error("failed to write edited tree")]
    TreeEditorWrite(#[source] gix::object::tree::editor::write::Error),
    #[error("failed to write commit")]
    CommitWrite(#[source] gix::object::write::Error),
    #[error("failed to create stash ref")]
    RefWrite(#[source] gix::reference::edit::Error),
    #[error("failed to delete stash ref")]
    RefDelete(#[source] gix::reference::edit::Error),
    #[error("failed to read index")]
    IndexRead(#[source] gix::worktree::open_index::Error),
    #[error("failed to write index")]
    IndexWrite(#[source] gix::index::file::write::Error),
    #[error("failed to read object")]
    ObjectFind(#[source] gix::object::find::existing::Error),
    #[error("failed to traverse tree")]
    TreeTraverse(#[source] gix::traverse::tree::breadthfirst::Error),
    #[error("non-UTF-8 path in tree")]
    NonUtf8Path,
    #[error("failed to determine committer identity; set user.name and user.email in git config")]
    NoIdentity,
    #[error("invalid committer time")]
    CommitterTime(#[source] gix::config::time::Error),
    #[error("invalid committer signature")]
    CommitterValidation(#[source] gix::date::Error),
    #[error("failed to read {path}")]
    FileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}")]
    FileWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Lock(#[from] gix::lock::acquire::Error),
    #[error("failed to delete {path}")]
    FileDelete {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to create merge resource cache")]
    MergeResourceCache(#[source] gix::repository::merge_resource_cache::Error),
    #[error("failed to load merge options")]
    BlobMergeOptions(#[source] gix::repository::blob_merge_options::Error),
    #[error("failed to load command context")]
    CommandContext(#[source] gix::config::command_context::Error),
    #[error("failed to create filter pipeline")]
    FilterPipeline(#[source] gix::repository::filter::pipeline::Error),
    #[error("failed to merge {path}")]
    MergeResource {
        path: String,
        #[source]
        source: gix::merge::blob::platform::set_resource::Error,
    },
    #[error("failed to merge {path}")]
    MergePrepare {
        path: String,
        #[source]
        source: gix::merge::blob::platform::prepare_merge::Error,
    },
    #[error("failed to convert {path} to worktree form")]
    ConvertToWorktree {
        path: String,
        #[source]
        source: gix::filter::pipeline::convert_to_worktree::Error,
    },
    #[error("failed to capture {path}")]
    Capture {
        path: String,
        #[source]
        source: gix::filter::pipeline::worktree_file_to_object::Error,
    },
    #[error("failed to compute repository status")]
    Status(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("failed to read stash reflog")]
    ReflogRead(#[source] gix::refs::file::log::Error),
    #[error("failed to load checkout options")]
    CheckoutOptions(#[source] gix::config::checkout_options::Error),
    #[error("failed to check out files")]
    Checkout(#[source] gix::worktree::state::checkout::Error),
    #[error("failed to open object database")]
    OdbOpen(#[source] std::io::Error),
    #[error("failed to load filesystem capabilities")]
    FsCapabilities(#[source] gix::config::boolean::Error),
    #[error("failed to load stat options")]
    StatOptions(#[source] gix::config::stat_options::Error),
    #[error("failed to read core.bigFileThreshold")]
    BigFileThreshold(#[source] gix::config::unsigned_integer::Error),
}

/// Owns the git state around one run: stash the working changes so commands see only staged
/// content, capture their output into the index, restore the working tree, and drop the backup
/// stash.
///
/// Constructing it performs the stash. If a later step fails before `restore` runs - a command
/// failure, or a mid-way hide failure during construction - `Drop` rolls the working tree back.
/// Once `restore` has run, any failure leaves the backup stash in place for recovery.
#[must_use]
pub struct Workflow<'a> {
    repo: &'a Repository,
    workdir: &'a Path,
    status: WorktreeStatus,
    stash_tracked: bool,
    oid: Option<ObjectId>,
    absent: Vec<String>,
    hidden: BTreeSet<String>, // dirty paths hidden from the run; all restore rewrites on success
    attempted: bool,          // restore() was called; prevents Drop from retrying on failure
    filter: gix::filter::Pipeline<'a>,
    filter_index: gix::worktree::IndexPersistedOrInMemory,
}

impl<'a> Workflow<'a> {
    /// Stash and hide the working-tree changes.
    ///
    /// `Self` is built only once the stash commit exists, so an earlier failure cannot trigger
    /// `Drop`'s rollback; a hide failure afterwards rolls back on drop.
    pub fn new(
        repo: &'a Repository,
        workdir: &'a Path,
        status: WorktreeStatus,
        stash_tracked: bool,
        stash_untracked: bool,
    ) -> Result<Self, Error> {
        let (mut filter, filter_index) =
            repo.filter_pipeline(None).map_err(Error::FilterPipeline)?;

        // Read working-tree content into the ODB before hiding overwrites it with the indexed
        // version. Every dirty file is captured so the stash snapshots the full worktree; only the
        // scope-selected subset is hidden from the run.
        let mut captured: Vec<StashEntry> = Vec::new();
        let mut untracked_entries: Vec<StashEntry> = Vec::new();
        let mut hidden: BTreeSet<String> = BTreeSet::new();

        for path in &status.dirty {
            let (oid, mode) = hash_blob(&mut filter, &filter_index, workdir, path)?;
            captured.push(StashEntry {
                path: path.clone(),
                oid,
                mode,
            });
            if stash_tracked || status.staged.contains(path) {
                hidden.insert(path.clone());
            }
        }

        if stash_untracked {
            for path in &status.untracked {
                let (oid, mode) = hash_blob(&mut filter, &filter_index, workdir, path)?;
                untracked_entries.push(StashEntry {
                    path: path.clone(),
                    oid,
                    mode,
                });
            }
        }

        // Commands need staged files on disk; tracked stashing also hides unstaged deletions.
        let absent: Vec<String> = if stash_tracked {
            status.missing.iter().cloned().collect()
        } else {
            status
                .staged
                .intersection(&status.missing)
                .cloned()
                .collect()
        };

        let oid = if captured.is_empty() && untracked_entries.is_empty() && absent.is_empty() {
            None
        } else {
            Some(create_stash_commit(
                repo,
                &captured,
                &untracked_entries,
                &status.missing,
                &filter_index,
            )?)
        };

        let workflow = Self {
            repo,
            workdir,
            status,
            stash_tracked,
            oid,
            absent,
            hidden,
            attempted: false,
            filter,
            filter_index,
        };
        if workflow.oid.is_some() {
            checkout_index(
                repo,
                workdir,
                &workflow.hidden,
                &untracked_entries,
                &workflow.absent,
                &workflow.filter_index,
            )?;
        }
        Ok(workflow)
    }

    /// Finish the run: capture its output into the index, restore the working tree, apply the
    /// changes to it, and drop the backup stash.
    pub fn finish(&mut self, quiet: bool) -> Result<(), Error> {
        let merge_bases = update_index(
            self.repo,
            self.workdir,
            &self.status.staged,
            &self.status.dirty,
            &mut self.filter,
            &self.filter_index,
        )?;
        self.restore(false)?;
        apply_merges(
            self.repo,
            self.workdir,
            quiet,
            &merge_bases,
            &mut self.filter,
        )?;
        self.cleanup()
    }

    /// Restore the working tree and revert side-effects on clean tracked files.
    ///
    /// If the stash apply fails, subsequent steps are skipped and the stash ref is left intact
    /// for `git stash pop` recovery.
    fn restore(&mut self, rollback: bool) -> Result<(), Error> {
        self.attempted = true;

        // Undo the materialization: these files must return to their deleted worktree state.
        for path in &self.absent {
            let file_path = self.workdir.join(path);
            remove_if_exists(&file_path).map_err(|e| Error::FileDelete {
                path: file_path,
                source: e,
            })?;
        }

        if let Some(oid) = self.oid {
            // Rollback returns every dirty file to its pre-run bytes; success only unhides.
            let manifest = if rollback {
                &self.status.dirty
            } else {
                &self.hidden
            };
            apply_stash(self.repo, self.workdir, oid, manifest)?;
        }

        // Rolling back also reverts side-effects in scopes that leave dirty files in place.
        if self.stash_tracked || rollback {
            let skip: HashSet<&str> = self
                .status
                .dirty
                .iter()
                .chain(&self.status.missing)
                .map(String::as_str)
                .collect();
            restore_clean_tracked(self.repo, self.workdir, &skip)?;
        }

        Ok(())
    }

    /// Drop the backup stash now that the run has succeeded.
    fn cleanup(&self) -> Result<(), Error> {
        if let Some(oid) = self.oid {
            drop_stash(self.repo, oid)?;
        }
        Ok(())
    }
}

impl Drop for Workflow<'_> {
    fn drop(&mut self) {
        // Once restore has run, any failure after it keeps the stash ref for recovery.
        if self.attempted {
            return;
        }
        if let Err(e) = self.restore(true) {
            eprintln!("stagelint: warning: {:#}", anyhow::Error::new(e));
        } else if let Err(e) = self.cleanup() {
            eprintln!(
                "stagelint: warning: failed to drop stash ref: {:#}",
                anyhow::Error::new(e)
            );
        }
    }
}

/// A file captured for stashing.
struct StashEntry {
    path: String,
    oid: ObjectId,
    mode: EntryMode,
}

impl StashEntry {
    /// Index modes with no tree form fall back to a plain blob.
    fn from_index_entry(path: String, entry: &gix::index::Entry) -> Self {
        StashEntry {
            path,
            oid: entry.id,
            mode: entry
                .mode
                .to_tree_entry_mode()
                .unwrap_or(EntryKind::Blob.into()),
        }
    }
}

/// Capture a worktree file into the ODB in git form (clean-filtered), as `git add` would.
fn hash_blob(
    filter: &mut gix::filter::Pipeline<'_>,
    filter_index: &gix::index::State,
    workdir: &Path,
    path: &str,
) -> Result<(ObjectId, EntryMode), Error> {
    match filter.worktree_file_to_object(path.as_bytes().as_bstr(), filter_index) {
        Ok(Some((oid, kind, _))) => Ok((oid, kind.into())),
        Ok(None) => Err(Error::FileRead {
            path: workdir.join(path),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        }),
        Err(e) => Err(Error::Capture {
            path: path.to_owned(),
            source: e,
        }),
    }
}

/// Write blobs to the worktree as `git checkout-index -f` would, honoring filters and modes.
fn checkout_worktree(
    repo: &Repository,
    workdir: &Path,
    entries: &[StashEntry],
) -> Result<(), Error> {
    if entries.is_empty() {
        return Ok(());
    }

    let mut state = gix::index::State::new(repo.object_hash());
    for entry in entries {
        state.dangerously_push_entry(
            entry::Stat::default(),
            entry.oid,
            entry::Flags::empty(),
            entry.mode.into(),
            entry.path.as_bytes().as_bstr(),
        );
    }
    // dangerously_push_entry requires sorted order, which our sources do not guarantee.
    state.sort_entries();

    let mut options = repo
        .checkout_options(gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping)
        .map_err(Error::CheckoutOptions)?;
    options.destination_is_initially_empty = false;
    options.overwrite_existing = true;

    gix::worktree::state::checkout(
        &mut state,
        workdir,
        repo.objects.clone().into_arc().map_err(Error::OdbOpen)?,
        &gix::progress::Discard,
        &gix::progress::Discard,
        &std::sync::atomic::AtomicBool::default(),
        options,
    )
    .map_err(Error::Checkout)?;
    Ok(())
}

/// Build the stash commit and write refs/stash (if HEAD exists).
fn create_stash_commit(
    repo: &Repository,
    captured: &[StashEntry],
    untracked_entries: &[StashEntry],
    missing: &BTreeSet<String>,
    index: &gix::index::State,
) -> Result<ObjectId, Error> {
    let committer = repo
        .committer()
        .ok_or(Error::NoIdentity)?
        .map_err(Error::CommitterTime)?
        .to_owned()
        .map_err(Error::CommitterValidation)?;

    let head = repo.head_commit().ok();

    // Build the w_tree: HEAD overlaid with every dirty file and all worktree deletions.
    let base_tree = match &head {
        Some(commit) => commit.tree().map_err(Error::TreeDecode)?,
        None => repo.empty_tree(),
    };
    let mut editor = base_tree.edit().map_err(Error::TreeEditInit)?;
    for entry in captured {
        editor
            .upsert(entry.path.as_str(), entry.mode.kind(), entry.oid)
            .map_err(Error::TreeEdit)?;
    }
    // Mirror git's w_tree so git stash pop can recover after a crash.
    for path in missing {
        if base_tree
            .lookup_entry_by_path(Path::new(path))
            .map_err(Error::ObjectFind)?
            .is_some()
        {
            editor.remove(path.as_str()).map_err(Error::TreeEdit)?;
        } else if let Some((id, kind)) = index
            .entry_by_path_and_stage(path.as_bytes().as_bstr(), Stage::Unconflicted)
            .and_then(|e| e.mode.to_tree_entry_mode().map(|m| (e.id, m.kind())))
        {
            editor
                .upsert(path.as_str(), kind, id)
                .map_err(Error::TreeEdit)?;
        }
    }
    let stash_tree_oid = editor.write().map_err(Error::TreeEditorWrite)?.detach();

    let mut parents: Vec<ObjectId> = Vec::new();
    if let Some(commit) = &head {
        parents.push(commit.id);

        // parent[1]: index commit - tree is the current staged state.
        // git stash pop uses this as the merge base for staged changes; using HEAD tree instead
        // would produce wrong 3-way merge results for partially-staged files.
        let index_tree_oid = {
            let mut editor = repo.empty_tree().edit().map_err(Error::TreeEditInit)?;
            for entry in index.entries() {
                if entry.flags.stage() != Stage::Unconflicted {
                    continue;
                }
                let path =
                    std::str::from_utf8(entry.path(index)).map_err(|_| Error::NonUtf8Path)?;
                let Some(mode) = entry.mode.to_tree_entry_mode() else {
                    continue;
                };
                editor
                    .upsert(path, mode.kind(), entry.id)
                    .map_err(Error::TreeEdit)?;
            }
            editor.write().map_err(Error::TreeEditorWrite)?.detach()
        };
        let index_commit_oid = write_commit(
            repo,
            &committer,
            index_tree_oid,
            &[commit.id],
            "stagelint: index on HEAD",
        )?;
        parents.push(index_commit_oid);
    }

    // parent[2]: untracked commit
    if !untracked_entries.is_empty() {
        let mut editor = repo.empty_tree().edit().map_err(Error::TreeEditInit)?;
        for entry in untracked_entries {
            editor
                .upsert(entry.path.as_str(), entry.mode.kind(), entry.oid)
                .map_err(Error::TreeEdit)?;
        }
        let untracked_tree_oid = editor.write().map_err(Error::TreeEditorWrite)?.detach();
        let untracked_commit_oid =
            write_commit(repo, &committer, untracked_tree_oid, &[], "untracked files")?;
        parents.push(untracked_commit_oid);
    }

    let commit_oid = write_commit(
        repo,
        &committer,
        stash_tree_oid,
        &parents,
        "stagelint automatic backup",
    )?;

    // Skip on empty repos - git stash pop can't handle orphan commits.
    if head.is_some() {
        repo.edit_reference(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: true,
                    message: "stagelint automatic backup".into(),
                },
                expected: PreviousValue::Any,
                new: Target::Object(commit_oid),
            },
            name: "refs/stash".try_into().expect("valid ref name"),
            deref: false,
        })
        .map_err(Error::RefWrite)?;
    }

    Ok(commit_oid)
}

fn write_commit(
    repo: &Repository,
    committer: &gix::actor::Signature,
    tree: ObjectId,
    parents: &[ObjectId],
    message: &str,
) -> Result<ObjectId, Error> {
    let commit = Commit {
        tree,
        parents: parents.to_vec().into(),
        author: committer.clone(),
        committer: committer.clone(),
        encoding: None,
        message: message.into(),
        extra_headers: vec![],
    };
    repo.write_object(&commit)
        .map(Id::detach)
        .map_err(Error::CommitWrite)
}

/// Check out indexed content over stashed files, as `git checkout-index` would.
/// For hidden tracked files: write indexed blob content.
/// For untracked files: delete them so they're absent during the run.
/// For absent staged files: write indexed content so they exist during the run.
fn checkout_index(
    repo: &Repository,
    workdir: &Path,
    hidden: &BTreeSet<String>,
    untracked_entries: &[StashEntry],
    staged_absent: &[String],
    index: &gix::index::State,
) -> Result<(), Error> {
    // Deletions first: an untracked file can occupy the parent path of an absent index entry.
    for entry in untracked_entries {
        let file_path = workdir.join(&entry.path);
        remove_if_exists(&file_path).map_err(|e| Error::FileDelete {
            path: file_path,
            source: e,
        })?;
    }

    // Hidden files get their indexed content; absent staged files are recreated from it so they
    // exist during the run (restore deletes them again).
    let mut to_write = Vec::new();
    for path in hidden.iter().chain(staged_absent) {
        let bpath = path.as_bytes().as_bstr();
        let Some(entry) = index.entry_by_path_and_stage(bpath, Stage::Unconflicted) else {
            continue;
        };
        to_write.push(StashEntry::from_index_entry(path.clone(), entry));
    }
    checkout_worktree(repo, workdir, &to_write)
}

/// Apply stashed files to the working tree, as `git stash apply` would.
///
/// Applied by name rather than by diff: content matching HEAD must still be unhidden.
fn apply_stash(
    repo: &Repository,
    workdir: &Path,
    stash_oid: ObjectId,
    manifest: &BTreeSet<String>,
) -> Result<(), Error> {
    let stash_obj = repo.find_object(stash_oid).map_err(Error::ObjectFind)?;
    let stash_commit = stash_obj.into_commit();
    let parents: Vec<ObjectId> = stash_commit.parent_ids().map(Id::detach).collect();
    let stash_tree_oid = stash_commit
        .tree_id()
        .map_err(|e| Error::TreeDecode(e.into()))?
        .detach();

    // Stash parents: [HEAD, index, untracked?], or [untracked?] on an empty repo (>=2 means HEAD+index).
    let has_head = parents.len() >= 2;
    apply_stash_tree(repo, workdir, stash_tree_oid, manifest)?;

    let untracked_idx = if has_head { 2 } else { 0 };
    if let Some(&untracked_oid) = parents.get(untracked_idx) {
        apply_stash_untracked(repo, workdir, untracked_oid)?;
    }

    Ok(())
}

/// Restore the manifest's paths from the stash tree; deletions are untouched.
fn apply_stash_tree(
    repo: &Repository,
    workdir: &Path,
    stash_tree_oid: ObjectId,
    manifest: &BTreeSet<String>,
) -> Result<(), Error> {
    let stash_tree = repo
        .find_object(stash_tree_oid)
        .map_err(Error::ObjectFind)?
        .into_tree();

    let mut to_write = Vec::new();
    for path in manifest {
        let Some(entry) = stash_tree
            .lookup_entry_by_path(Path::new(path))
            .map_err(Error::ObjectFind)?
        else {
            continue;
        };
        let mode = entry.mode();
        if mode.is_tree() {
            continue;
        }
        to_write.push(StashEntry {
            path: path.clone(),
            oid: entry.id().detach(),
            mode,
        });
    }
    checkout_worktree(repo, workdir, &to_write)
}

/// Restore untracked files stored in the stash's third-parent commit.
fn apply_stash_untracked(
    repo: &Repository,
    workdir: &Path,
    untracked_commit_oid: ObjectId,
) -> Result<(), Error> {
    let untracked_tree_oid = repo
        .find_object(untracked_commit_oid)
        .map_err(Error::ObjectFind)?
        .into_commit()
        .tree_id()
        .map_err(|e| Error::TreeDecode(e.into()))?
        .detach();
    let untracked_tree = repo
        .find_object(untracked_tree_oid)
        .map_err(Error::ObjectFind)?
        .into_tree();
    let mut to_write = Vec::new();
    for entry in untracked_tree
        .traverse()
        .breadthfirst
        .files()
        .map_err(Error::TreeTraverse)?
    {
        if entry.mode.is_tree() {
            continue;
        }
        let path = entry.filepath.to_str().map_err(|_| Error::NonUtf8Path)?;
        to_write.push(StashEntry {
            path: path.to_owned(),
            oid: entry.oid,
            mode: entry.mode,
        });
    }
    checkout_worktree(repo, workdir, &to_write)
}

/// Remove our stash entry from the reflog and update/delete refs/stash.
fn drop_stash(repo: &Repository, oid: ObjectId) -> Result<(), Error> {
    let reflog_path = repo.common_dir().join("logs/refs/stash");
    if !reflog_path.exists() {
        return Ok(());
    }

    // All git stash operations run under refs/stash.lock; hold it for the whole rewrite.
    let ref_path = repo.common_dir().join("refs/stash");
    let mut ref_lock =
        gix::lock::File::acquire_to_update_resource(&ref_path, Fail::Immediately, None)?;

    let mut reflog_lock =
        gix::lock::File::acquire_to_update_resource(&reflog_path, Fail::Immediately, None)?;
    let mut buf = Vec::new();
    let Some(lines) = repo
        .refs
        .reflog_iter("refs/stash", &mut buf)
        .map_err(Error::ReflogRead)?
    else {
        return Ok(());
    };

    let mut last_oid: Option<ObjectId> = None;

    for line in lines {
        let Ok(parsed) = line else {
            continue;
        };
        let mut entry = Line::from(parsed);
        if entry.new_oid == oid {
            continue;
        }
        // The chain is recomputed, not preserved: each old_oid becomes the previous kept entry.
        entry.previous_oid = last_oid.unwrap_or_else(|| ObjectId::null(repo.object_hash()));
        entry
            .write_to(&mut reflog_lock)
            .map_err(|e| Error::FileWrite {
                path: reflog_lock.lock_path().to_path_buf(),
                source: e,
            })?;
        last_oid = Some(entry.new_oid);
    }

    reflog_lock.commit().map_err(|e| Error::FileWrite {
        path: reflog_path,
        source: e.error,
    })?;

    if let Some(last) = last_oid {
        ref_lock
            .write_all(format!("{}\n", last.to_hex()).as_bytes())
            .map_err(|e| Error::FileWrite {
                path: ref_lock.lock_path().to_path_buf(),
                source: e,
            })?;
        ref_lock.commit().map_err(|e| Error::FileWrite {
            path: ref_path,
            source: e.error,
        })?;
        Ok(())
    } else {
        drop(ref_lock);
        match repo.edit_reference(RefEdit {
            change: Change::Delete {
                expected: PreviousValue::MustExistAndMatch(Target::Object(oid)),
                log: RefLog::AndReference,
            },
            name: "refs/stash".try_into().expect("valid ref name"),
            deref: false,
        }) {
            Ok(_)
            | Err(gix::reference::edit::Error::FileTransactionPrepare(
                gix::refs::file::transaction::prepare::Error::ReferenceOutOfDate { .. }
                | gix::refs::file::transaction::prepare::Error::DeleteReferenceMustExist { .. },
            )) => Ok(()),
            Err(e) => Err(Error::RefDelete(e)),
        }
    }
}

/// A partially-staged file that changed during the run.
struct MergeBase {
    path: String,
    /// Original staged content before the run.
    base_oid: ObjectId,
    /// Index content after the run.
    after_oid: ObjectId,
}

/// Single-pass: detect changes made during the run, update index OIDs in place, return merge bases.
///
/// For each staged file: checks index stat first (skip if reliably clean), then streams to ODB. If
/// the OID changed, updates the entry in place. If also partially staged, records a `MergeBase`.
/// Writes the index once at the end.
fn update_index(
    repo: &Repository,
    workdir: &Path,
    staged: &BTreeSet<String>,
    dirty: &BTreeSet<String>,
    filter: &mut gix::filter::Pipeline<'_>,
    filter_index: &gix::index::State,
) -> Result<Vec<MergeBase>, Error> {
    // Hold index.lock across the read-modify-write, as git does: a concurrent writer can neither
    // clobber this update nor be clobbered by it.
    let lock =
        gix::lock::File::acquire_to_update_resource(repo.index_path(), Fail::Immediately, None)?;
    let mut index = repo.open_index().map_err(Error::IndexRead)?;
    let caps = repo.filesystem_options().map_err(Error::FsCapabilities)?;
    let stat_options = repo.stat_options().map_err(Error::StatOptions)?;
    let mut merge_bases = Vec::new();
    let mut changed = false;

    for path in staged {
        let file_path = workdir.join(path);
        let bpath = path.as_bytes().as_bstr();
        let Some(pos) = index.entry_index_by_path_and_stage(bpath, Stage::Unconflicted) else {
            continue;
        };

        let meta = match Metadata::from_path_no_follow(&file_path) {
            Ok(meta) => meta,
            // Deleted by the run: stage the removal, as `git add` would.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                index.remove_entry_at_index(pos);
                changed = true;
                continue;
            }
            Err(e) => {
                return Err(Error::FileRead {
                    path: file_path,
                    source: e,
                });
            }
        };

        // Mode before stat, as `git status` does: a chmod only changes ctime, so with
        // `core.trustctime=false` a clean stat would hide it.
        let mode_change =
            index.entries()[pos]
                .mode
                .change_to_match_fs(&meta, caps.symlink, caps.executable_bit);

        let entry_stat = index.entries()[pos].stat;
        if mode_change.is_none()
            && entry::Stat::from_fs(&meta).ok().is_some_and(|s| {
                s.matches(&entry_stat, stat_options)
                    && !entry_stat.is_racy(index.timestamp(), stat_options)
            })
        {
            continue;
        }

        let original_oid = index.entries()[pos].id;
        let (new_oid, _) = hash_blob(filter, filter_index, workdir, path)?;

        if new_oid != original_oid || mode_change.is_some() {
            if new_oid != original_oid && dirty.contains(path) {
                merge_bases.push(MergeBase {
                    path: path.clone(),
                    base_oid: original_oid,
                    after_oid: new_oid,
                });
            }
            let entry = &mut index.entries_mut()[pos];
            entry.id = new_oid;
            if let Some(change) = mode_change {
                entry.mode = change.apply(entry.mode);
            }
            if let Ok(stat) = entry::Stat::from_fs(&meta) {
                entry.stat = stat;
            }
            changed = true;
        }
    }

    if changed {
        // gix writes the tree cache as-is, without invalidating it for modified entries; drop it
        // so a later commit cannot reuse stale subtrees. This is the documented practice until a
        // gix-index API rework: https://github.com/GitoxideLabs/gitoxide/issues/2421
        index.remove_tree();
        let mut writer = std::io::BufWriter::with_capacity(64 * 1024, lock);
        index
            .write_to(&mut writer, write::Options::default())
            .map_err(|e| Error::IndexWrite(e.into()))?;
        match writer.into_inner() {
            Ok(lock) => lock.commit().map_err(|e| Error::IndexWrite(e.into()))?,
            Err(e) => {
                return Err(Error::FileWrite {
                    path: repo.index_path(),
                    source: e.into_error(),
                });
            }
        };
    }

    Ok(merge_bases)
}

/// Apply the captured changes to the restored working tree via three-way merge.
///
/// Merging honors gitattributes, including merge drivers and binary detection. Changes reach
/// the worktree only when the merge is clean; otherwise the file is left untouched and only
/// the commit carries the new content.
fn apply_merges(
    repo: &Repository,
    workdir: &Path,
    quiet: bool,
    merge_bases: &[MergeBase],
    filter: &mut gix::filter::Pipeline<'_>,
) -> Result<(), Error> {
    if merge_bases.is_empty() {
        return Ok(());
    }

    let threshold = repo.big_file_threshold().map_err(Error::BigFileThreshold)?;
    let roots = WorktreeRoots {
        current_root: Some(workdir.to_path_buf()),
        ..Default::default()
    };
    let mut platform = repo
        .merge_resource_cache(roots)
        .map_err(Error::MergeResourceCache)?;
    let options = repo.blob_merge_options().map_err(Error::BlobMergeOptions)?;
    let context = repo.command_context().map_err(Error::CommandContext)?;

    let null = ObjectId::null(repo.object_hash());
    let mut out = Vec::new();
    for mb in merge_bases {
        let file_path = workdir.join(&mb.path);
        // Only merge regular files; anything else would read and write through symlinks.
        let Ok(meta) = file_path.symlink_metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }

        let file_size = meta.len();
        if file_size > threshold {
            if !quiet {
                eprintln!(
                    "stagelint: warning: could not apply changes to working tree for {}: file \
                     exceeds core.bigFileThreshold - staged content is updated, working tree \
                     unchanged",
                    mb.path
                );
            }
            continue;
        }

        let rela = mb.path.as_bytes().as_bstr();
        let sides = [
            (null, ResourceKind::CurrentOrOurs),
            (mb.base_oid, ResourceKind::CommonAncestorOrBase),
            (mb.after_oid, ResourceKind::OtherOrTheirs),
        ];
        for (id, kind) in sides {
            platform
                .set_resource(id, EntryKind::Blob, rela, kind, &repo.objects)
                .map_err(|e| Error::MergeResource {
                    path: mb.path.clone(),
                    source: e,
                })?;
        }
        let prepared = platform
            .prepare_merge(&repo.objects, options)
            .map_err(|e| Error::MergePrepare {
                path: mb.path.clone(),
                source: e,
            })?;

        out.clear();
        // A failing merge driver counts as a conflict, matching git.
        let (pick, resolution) = match prepared.merge(&mut out, Labels::default(), &context) {
            Ok(result) => result,
            Err(e) => {
                if !quiet {
                    eprintln!(
                        "stagelint: warning: could not apply all changes to {}: {:#}",
                        mb.path,
                        anyhow::Error::new(e)
                    );
                }
                continue;
            }
        };
        if resolution != Resolution::Complete {
            if !quiet {
                eprintln!(
                    "stagelint: warning: could not apply changes to {}: they conflict with unstaged changes",
                    mb.path
                );
            }
            continue;
        }
        // Ours means the worktree file already holds the result.
        if matches!(pick, Pick::Ours) {
            continue;
        }
        let merged = match prepared.buffer_by_pick(pick) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => out.as_slice(),
            Err(()) => {
                if !quiet {
                    eprintln!(
                        "stagelint: warning: could not apply changes to {}: merge result unavailable",
                        mb.path
                    );
                }
                continue;
            }
        };

        write_merged(filter, merged, rela, &file_path)?;
    }

    Ok(())
}

/// Convert a merge result from git form back to worktree form and write it.
fn write_merged(
    filter: &mut gix::filter::Pipeline<'_>,
    merged: &[u8],
    rela_path: &gix::bstr::BStr,
    file_path: &Path,
) -> Result<(), Error> {
    let mut converted = filter
        .convert_to_worktree(
            merged,
            rela_path,
            to_worktree::Options {
                can_delay: Delay::Forbid,
                ..Default::default()
            },
        )
        .map_err(|e| Error::ConvertToWorktree {
            path: rela_path.to_string(),
            source: e,
        })?;
    let write_err = |e: std::io::Error| Error::FileWrite {
        path: file_path.to_path_buf(),
        source: e,
    };
    if let Some(bytes) = converted.as_bytes() {
        fs::write(file_path, bytes).map_err(write_err)?;
    } else if let Some(read) = converted.as_read() {
        let mut file = fs::File::create(file_path).map_err(write_err)?;
        std::io::copy(read, &mut file).map_err(write_err)?;
    }
    Ok(())
}

/// Restore tracked files modified as a side-effect during the run (not staged, but changed on
/// disk) from the index, as `git restore` would. Skips any paths in `skip`.
fn restore_clean_tracked(
    repo: &Repository,
    workdir: &Path,
    skip: &HashSet<&str>,
) -> Result<(), Error> {
    let iter = repo
        .status(gix::progress::Discard)
        .map_err(|e| Error::Status(Box::new(e)))?
        .into_index_worktree_iter(Vec::<gix::bstr::BString>::new())
        .map_err(|e| Error::Status(Box::new(e)))?;

    let mut to_write = Vec::new();

    for item in iter {
        let item = item.map_err(|e| Error::Status(Box::new(e)))?;
        let gix::status::index_worktree::Item::Modification {
            entry,
            rela_path,
            status,
            ..
        } = item
        else {
            continue;
        };
        if !matches!(status, EntryStatus::Change(_)) {
            continue;
        }

        // Indexed symlink data is the target path, not file content; submodules have none at all.
        if matches!(entry.mode, entry::Mode::SYMLINK | entry::Mode::COMMIT) {
            continue;
        }

        // Skipping an undecodable bystander beats failing the whole rollback over it.
        let Ok(path) = std::str::from_utf8(&rela_path) else {
            continue;
        };
        if skip.contains(path) {
            continue;
        }

        to_write.push(StashEntry::from_index_entry(path.to_owned(), &entry));
    }

    checkout_worktree(repo, workdir, &to_write)
}

fn remove_if_exists(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // On Windows, remove_file fails on directory symlinks - use remove_dir for those.
        match path.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
            // FILE_ATTRIBUTE_DIRECTORY: set on the link itself, even when dangling.
            Ok(m) if m.file_attributes() & 0x10 != 0 => return fs::remove_dir(path),
            Ok(_) => {}
        }
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}
