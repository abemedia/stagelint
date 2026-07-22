use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use gix::index::entry::{self, Stage};
use gix::index::{fs::Metadata, write};
use gix::lock::acquire::Fail;
use gix::objs::Commit;
use gix::objs::tree::{EntryKind, EntryMode};
use gix::refs::Target;
use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
use gix::{Id, ObjectId, Repository};

use crate::merge;
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
    IndexRead(#[source] gix::index::file::init::Error),
    #[error("failed to write index")]
    IndexWrite(#[source] gix::index::file::write::Error),
    #[error("failed to read object")]
    ObjectFind(#[source] gix::object::find::existing::Error),
    #[error("failed to diff trees")]
    TreeDiff(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("non-utf8 path in tree")]
    NonUtf8Path,
    #[error("could not determine committer identity; set user.name and user.email in git config")]
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
    #[error("failed to write blob for {path}")]
    BlobWrite {
        path: String,
        #[source]
        source: gix::object::write::Error,
    },
    #[error("failed to read blob for {path}")]
    BlobRead {
        path: String,
        #[source]
        source: gix::object::find::existing::Error,
    },
    #[error("failed to merge {path}")]
    Merge {
        path: String,
        #[source]
        source: merge::Error,
    },
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
        // Read working-tree content into the ODB before hiding overwrites it with the indexed
        // version. Every dirty file is captured so the stash snapshots the full worktree; only the
        // scope-selected subset is hidden from the run.
        let mut captured: Vec<StashEntry> = Vec::new();
        let mut untracked_entries: Vec<StashEntry> = Vec::new();
        let mut hidden: BTreeSet<String> = BTreeSet::new();

        for path in &status.dirty {
            let (oid, mode) = read_blob(repo, workdir, path)?;
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
                let (oid, mode) = read_blob(repo, workdir, path)?;
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
        };
        if workflow.oid.is_some() {
            hide_stashed_files(
                repo,
                workdir,
                &workflow.hidden,
                &untracked_entries,
                &workflow.absent,
            )?;
        }
        Ok(workflow)
    }

    /// Finish the run: capture its output into the index, restore the working tree, apply the
    /// changes to it, and drop the backup stash.
    pub fn finish(&mut self, quiet: bool) -> Result<(), Error> {
        let merge_bases = update(
            self.repo,
            self.workdir,
            &self.status.staged,
            &self.status.dirty,
        )?;
        self.restore(false)?;
        apply_merges(self.repo, self.workdir, quiet, &merge_bases)?;
        self.cleanup()
    }

    /// Restore the working tree, revert side-effects on clean tracked files, and refresh index
    /// stats so restored files don't appear dirty.
    ///
    /// If stash restore fails, subsequent steps are skipped (they depend on knowing which files
    /// were restored). The stash ref is left intact for `git stash pop` recovery.
    fn restore(&mut self, rollback: bool) -> Result<(), Error> {
        self.attempted = true;

        let paths = if let Some(oid) = self.oid {
            // Rollback returns every dirty file to its pre-run bytes; success only unhides.
            let manifest = if rollback {
                &self.status.dirty
            } else {
                &self.hidden
            };
            restore_stashed(self.repo, self.workdir, oid, manifest, &self.hidden)?
        } else {
            Vec::new()
        };

        // Undo the materialization: these files must return to their deleted worktree state.
        for path in &self.absent {
            let file_path = self.workdir.join(path);
            remove_if_exists(&file_path).map_err(|e| Error::FileDelete {
                path: file_path,
                source: e,
            })?;
        }

        // Rolling back also reverts side-effects in scopes that leave dirty files in place.
        if self.stash_tracked || rollback {
            let mut skip = paths.clone();
            skip.extend(self.status.dirty.iter().cloned());
            skip.extend(self.status.missing.iter().cloned());
            revert_clean_tracked(self.repo, self.workdir, &skip)?;
        }

        if self.oid.is_some() {
            refresh_stat(self.repo, self.workdir, &paths)?;
        }

        Ok(())
    }

    /// Drop the backup stash now that the run has succeeded.
    fn cleanup(&self) -> Result<(), Error> {
        if let Some(oid) = self.oid {
            remove_stash_ref(self.repo, oid)?;
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

/// Read a file's content into the ODB and return its OID + tree entry mode.
fn read_blob(
    repo: &Repository,
    workdir: &Path,
    path: &str,
) -> Result<(ObjectId, EntryMode), Error> {
    let file_path = workdir.join(path);
    let meta = file_path.symlink_metadata().map_err(|e| Error::FileRead {
        path: file_path.clone(),
        source: e,
    })?;

    let is_symlink = meta.file_type().is_symlink();

    let mode = if is_symlink {
        EntryKind::Link.into()
    } else if is_executable(&meta) {
        EntryKind::BlobExecutable.into()
    } else {
        EntryKind::Blob.into()
    };

    let content = if is_symlink {
        let target = fs::read_link(&file_path).map_err(|e| Error::FileRead {
            path: file_path.clone(),
            source: e,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            target.as_os_str().as_bytes().to_vec()
        }
        #[cfg(not(unix))]
        target.to_string_lossy().into_owned().into_bytes()
    } else {
        fs::read(&file_path).map_err(|e| Error::FileRead {
            path: file_path.clone(),
            source: e,
        })?
    };

    let oid = repo
        .write_blob(&content)
        .map_err(|e| Error::BlobWrite {
            path: path.to_owned(),
            source: e,
        })?
        .detach();

    Ok((oid, mode))
}

/// Build the stash commit and write refs/stash (if HEAD exists).
fn create_stash_commit(
    repo: &Repository,
    captured: &[StashEntry],
    untracked_entries: &[StashEntry],
    missing: &BTreeSet<String>,
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
    let index = open_index(repo).map_err(Error::IndexRead)?;
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
        } else if let Some(entry) =
            index.entry_by_path_and_stage(path.as_bytes().as_bstr(), Stage::Unconflicted)
            && let Some(mode) = entry.mode.to_tree_entry_mode()
        {
            editor
                .upsert(path.as_str(), mode.kind(), entry.id)
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
                    std::str::from_utf8(entry.path(&index)).map_err(|_| Error::NonUtf8Path)?;
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

/// Replace stashed files with their correct on-disk representation.
/// For hidden tracked files: write indexed blob content.
/// For untracked files: delete them so they're absent during the run.
/// For absent staged files: write indexed content so they exist during the run.
fn hide_stashed_files(
    repo: &Repository,
    workdir: &Path,
    hidden: &BTreeSet<String>,
    untracked_entries: &[StashEntry],
    staged_absent: &[String],
) -> Result<(), Error> {
    let use_symlinks = repo
        .config_snapshot()
        .boolean("core.symlinks")
        .unwrap_or(cfg!(unix));
    let index = open_index(repo).map_err(Error::IndexRead)?;

    for path in hidden {
        let bpath = path.as_bytes().as_bstr();
        let Some(entry) = index.entry_by_path_and_stage(bpath, Stage::Unconflicted) else {
            continue;
        };
        let blob = repo.find_object(entry.id).map_err(|e| Error::BlobRead {
            path: path.clone(),
            source: e,
        })?;
        let file_path = workdir.join(path);
        let mode = entry
            .mode
            .to_tree_entry_mode()
            .unwrap_or(EntryKind::Blob.into());
        write_to_workdir(&file_path, &blob.data, mode, use_symlinks).map_err(|e| {
            Error::FileWrite {
                path: file_path.clone(),
                source: e,
            }
        })?;
    }

    for entry in untracked_entries {
        let file_path = workdir.join(&entry.path);
        remove_if_exists(&file_path).map_err(|e| Error::FileDelete {
            path: file_path,
            source: e,
        })?;
    }

    // Recreate absent staged files from indexed content so they exist during the run; restore
    // deletes them.
    for path in staged_absent {
        let bpath = path.as_bytes().as_bstr();
        let Some(entry) = index.entry_by_path_and_stage(bpath, Stage::Unconflicted) else {
            continue;
        };
        let blob = repo.find_object(entry.id).map_err(|e| Error::BlobRead {
            path: path.clone(),
            source: e,
        })?;
        let file_path = workdir.join(path);
        let mode = entry
            .mode
            .to_tree_entry_mode()
            .unwrap_or(EntryKind::Blob.into());
        write_to_workdir(&file_path, &blob.data, mode, use_symlinks).map_err(|e| {
            Error::FileWrite {
                path: file_path.clone(),
                source: e,
            }
        })?;
    }

    Ok(())
}

/// Restore stashed files to the working tree.
///
/// Restored by name rather than by diff: content matching HEAD must still be unhidden.
///
/// Returns the hidden paths that were written, for `refresh_stat`.
fn restore_stashed(
    repo: &Repository,
    workdir: &Path,
    stash_oid: ObjectId,
    manifest: &BTreeSet<String>,
    hidden: &BTreeSet<String>,
) -> Result<Vec<String>, Error> {
    let use_symlinks = repo
        .config_snapshot()
        .boolean("core.symlinks")
        .unwrap_or(cfg!(unix));

    let stash_obj = repo.find_object(stash_oid).map_err(Error::ObjectFind)?;
    let stash_commit = stash_obj.into_commit();
    let parents: Vec<ObjectId> = stash_commit.parent_ids().map(Id::detach).collect();
    let stash_tree_oid = stash_commit
        .tree_id()
        .map_err(|e| Error::TreeDecode(e.into()))?
        .detach();

    // Stash parents: [HEAD, index, untracked?], or [untracked?] on an empty repo (>=2 means HEAD+index).
    let has_head = parents.len() >= 2;
    let mut restored = restore_stash_files(
        repo,
        workdir,
        stash_tree_oid,
        manifest,
        hidden,
        use_symlinks,
    )?;

    let untracked_idx = if has_head { 2 } else { 0 };
    if let Some(&untracked_oid) = parents.get(untracked_idx) {
        restore_untracked_files(repo, workdir, untracked_oid, &mut restored, use_symlinks)?;
    }

    Ok(restored)
}

/// Restore stash tree files that differ from HEAD to the working tree.
fn restore_stash_files(
    repo: &Repository,
    workdir: &Path,
    stash_tree_oid: ObjectId,
    manifest: &BTreeSet<String>,
    hidden: &BTreeSet<String>,
    use_symlinks: bool,
) -> Result<Vec<String>, Error> {
    let stash_tree = repo
        .find_object(stash_tree_oid)
        .map_err(Error::ObjectFind)?
        .into_tree();

    let mut restored = Vec::new();
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
        let blob = repo.find_object(entry.id()).map_err(Error::ObjectFind)?;
        let file_path = workdir.join(path);
        write_to_workdir(&file_path, &blob.data, mode, use_symlinks).map_err(|e| {
            Error::FileWrite {
                path: file_path,
                source: e,
            }
        })?;
        // Only hidden paths feed refresh_stat: other manifest paths still differ from the index.
        if hidden.contains(path) {
            restored.push(path.clone());
        }
    }

    Ok(restored)
}

/// Restore untracked files stored in the stash's third-parent commit.
fn restore_untracked_files(
    repo: &Repository,
    workdir: &Path,
    untracked_commit_oid: ObjectId,
    restored: &mut Vec<String>,
    use_symlinks: bool,
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
    for entry in untracked_tree
        .traverse()
        .breadthfirst
        .files()
        .map_err(|e| Error::TreeDiff(Box::new(e)))?
    {
        if entry.mode.is_tree() {
            continue;
        }
        let path = entry.filepath.to_str().map_err(|_| Error::NonUtf8Path)?;
        let blob = repo.find_object(entry.oid).map_err(Error::ObjectFind)?;
        let file_path = workdir.join(path);
        write_to_workdir(&file_path, &blob.data, entry.mode, use_symlinks).map_err(|e| {
            Error::FileWrite {
                path: file_path.clone(),
                source: e,
            }
        })?;
        restored.push(path.to_owned());
    }
    Ok(())
}

/// Remove our stash entry from the reflog and update/delete refs/stash.
fn remove_stash_ref(repo: &Repository, oid: ObjectId) -> Result<(), Error> {
    let reflog_path = repo.common_dir().join("logs/refs/stash");
    if !reflog_path.exists() {
        return Ok(());
    }
    let mut reflog_lock =
        gix::lock::File::acquire_to_update_resource(&reflog_path, Fail::Immediately, None)?;
    let reflog_file = match fs::File::open(&reflog_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(Error::FileRead {
                path: reflog_path,
                source: e,
            });
        }
    };

    let our_hex = oid.to_hex().to_string();
    let mut last_hex = None;

    // Stream reflog into the lock file, skipping our entry.
    // Each line: "<old_oid> <new_oid> <signature>\t<message>"
    for line in BufReader::new(reflog_file).lines() {
        let line = line.map_err(|e| Error::FileRead {
            path: reflog_path.clone(),
            source: e,
        })?;
        if line.is_empty() {
            continue;
        }
        let new_hex = line.split(' ').nth(1);
        if new_hex != Some(our_hex.as_str()) {
            writeln!(reflog_lock, "{line}").map_err(|e| Error::FileWrite {
                path: reflog_lock.lock_path().to_path_buf(),
                source: e,
            })?;
            last_hex = new_hex.map(str::to_owned);
        }
    }

    if let Some(hex) = last_hex {
        let ref_path = repo.common_dir().join("refs/stash");
        let mut ref_lock =
            gix::lock::File::acquire_to_update_resource(&ref_path, Fail::Immediately, None)?;
        ref_lock
            .write_all(format!("{hex}\n").as_bytes())
            .map_err(|e| Error::FileWrite {
                path: ref_lock.lock_path().to_path_buf(),
                source: e,
            })?;
        ref_lock.commit().map_err(|e| Error::FileWrite {
            path: ref_path,
            source: e.error,
        })?;
        reflog_lock.commit().map_err(|e| Error::FileWrite {
            path: reflog_path,
            source: e.error,
        })?;
    } else {
        // No remaining entries: delete the ref and reflog entirely.
        if let Ok(r) = repo.find_reference("refs/stash") {
            r.delete().map_err(Error::RefDelete)?;
        }
        fs::remove_file(&reflog_path).ok();
    }

    Ok(())
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
fn update(
    repo: &Repository,
    workdir: &Path,
    staged: &BTreeSet<String>,
    dirty: &BTreeSet<String>,
) -> Result<Vec<MergeBase>, Error> {
    let mut index = open_index(repo).map_err(Error::IndexRead)?;
    let mut merge_bases = Vec::new();
    let mut changed = false;

    for path in staged {
        let file_path = workdir.join(path);
        if !file_path.exists() {
            continue;
        }

        let bpath = path.as_bytes().as_bstr();
        let Some(pos) = index.entry_index_by_path_and_stage(bpath, Stage::Unconflicted) else {
            continue;
        };

        if stat_is_clean(
            &file_path,
            &index.entries()[pos].stat,
            index.timestamp().seconds(),
        ) {
            continue;
        }

        let original_oid = index.entries()[pos].id;

        let file = fs::File::open(&file_path).map_err(|e| Error::FileRead {
            path: file_path.clone(),
            source: e,
        })?;
        let new_oid = repo
            .write_blob_stream(file)
            .map_err(|e| Error::BlobWrite {
                path: path.to_owned(),
                source: e,
            })?
            .detach();

        if new_oid != original_oid {
            if dirty.contains(path) {
                merge_bases.push(MergeBase {
                    path: path.clone(),
                    base_oid: original_oid,
                    after_oid: new_oid,
                });
            }
            let entry = &mut index.entries_mut()[pos];
            entry.id = new_oid;
            if let Some(stat) = Metadata::from_path_no_follow(&file_path)
                .ok()
                .and_then(|m| entry::Stat::from_fs(&m).ok())
            {
                entry.stat = stat;
            }
            changed = true;
        }
    }

    if changed {
        index
            .write(write::Options::default())
            .map_err(Error::IndexWrite)?;
    }

    Ok(merge_bases)
}

/// Apply the captured changes to the restored working tree via three-way merge.
fn apply_merges(
    repo: &Repository,
    workdir: &Path,
    quiet: bool,
    merge_bases: &[MergeBase],
) -> Result<(), Error> {
    let threshold = big_file_threshold(repo);

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

        let base = repo.find_object(mb.base_oid).map_err(|e| Error::BlobRead {
            path: mb.path.clone(),
            source: e,
        })?;
        let after = repo
            .find_object(mb.after_oid)
            .map_err(|e| Error::BlobRead {
                path: mb.path.clone(),
                source: e,
            })?;
        let working = fs::read(&file_path).map_err(|e| Error::FileRead {
            path: file_path.clone(),
            source: e,
        })?;

        match merge::three_way_merge(workdir, &mb.path, &working, &base.data, &after.data) {
            Ok(()) => {}
            Err(merge::Error::AutoResolved) => {
                if !quiet {
                    eprintln!(
                        "stagelint: warning: could not apply all changes to {}",
                        mb.path
                    );
                }
            }
            Err(e) => {
                return Err(Error::Merge {
                    path: mb.path.clone(),
                    source: e,
                });
            }
        }
    }

    Ok(())
}

/// Returns `core.bigFileThreshold` in bytes (default 512 MiB).
fn big_file_threshold(repo: &Repository) -> u64 {
    repo.config_snapshot()
        .integer("core.bigFileThreshold")
        .and_then(|v| u64::try_from(v).ok())
        .unwrap_or(512 * 1024 * 1024)
}

/// Revert tracked files modified as a side-effect during the run (not staged, but changed on disk).
/// Skips any paths in `already_restored`.
fn revert_clean_tracked(
    repo: &Repository,
    workdir: &Path,
    already_restored: &[String],
) -> Result<(), Error> {
    let index = open_index(repo).map_err(Error::IndexRead)?;
    let skip: std::collections::HashSet<&str> =
        already_restored.iter().map(String::as_str).collect();

    for entry in index.entries() {
        let Ok(path) = std::str::from_utf8(entry.path(&index)) else {
            continue;
        };
        if skip.contains(path) {
            continue;
        }

        // Symlinks: indexed data is the target path, not file content. std::fs::read follows the
        // symlink and would compare/overwrite the target file - skip them entirely.
        if matches!(entry.mode, entry::Mode::SYMLINK | entry::Mode::COMMIT) {
            continue;
        }

        let file_path = workdir.join(path);
        if !file_path.exists() {
            // Deleted as a side-effect: user deletions are excluded via `already_restored`.
            let indexed = repo.find_object(entry.id).map_err(Error::ObjectFind)?;
            let mode = entry
                .mode
                .to_tree_entry_mode()
                .unwrap_or(EntryKind::Blob.into());
            write_to_workdir(&file_path, &indexed.data, mode, false).map_err(|e| {
                Error::FileWrite {
                    path: file_path,
                    source: e,
                }
            })?;
            continue;
        }

        if stat_is_clean(&file_path, &entry.stat, index.timestamp().seconds()) {
            continue;
        }

        let current = fs::read(&file_path).map_err(|e| Error::FileRead {
            path: file_path.clone(),
            source: e,
        })?;
        let indexed = repo.find_object(entry.id).map_err(Error::ObjectFind)?;
        if current != indexed.data {
            fs::write(&file_path, &indexed.data).map_err(|e| Error::FileWrite {
                path: file_path,
                source: e,
            })?;
        }
    }

    Ok(())
}

/// Re-stat restored files and update their index entries so `git status` doesn't flag them as dirty
/// after we overwrote them.
fn refresh_stat(repo: &Repository, workdir: &Path, paths: &[String]) -> Result<(), Error> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut index = open_index(repo).map_err(Error::IndexRead)?;
    let mut changed = false;

    for path in paths {
        let bpath = path.as_bytes().as_bstr();
        let Some(pos) = index.entry_index_by_path_and_stage(bpath, Stage::Unconflicted) else {
            continue;
        };
        let file_path = workdir.join(path);
        let Ok(meta) = Metadata::from_path_no_follow(&file_path) else {
            continue;
        };
        let entry = &mut index.entries_mut()[pos];
        let Ok(stat) = entry::Stat::from_fs(&meta) else {
            continue;
        };
        entry.stat = stat;
        changed = true;
    }

    if changed {
        index
            .write(write::Options::default())
            .map_err(Error::IndexWrite)?;
    }

    Ok(())
}

/// Returns `true` when the on-disk stat matches `entry_stat` and can be trusted. Requires mtime to
/// predate `index_write_secs` to guard against the racily-clean case on coarse-grained filesystems
/// (HFS+ 1-second mtime).
fn stat_is_clean(path: &Path, entry_stat: &entry::Stat, index_write_secs: i64) -> bool {
    Metadata::from_path_no_follow(path)
        .ok()
        .and_then(|m| entry::Stat::from_fs(&m).ok())
        .is_some_and(|s| s == *entry_stat && i64::from(entry_stat.mtime.secs) < index_write_secs)
}

fn open_index(repo: &Repository) -> Result<gix::index::File, gix::index::file::init::Error> {
    gix::index::File::at(
        repo.index_path(),
        repo.object_hash(),
        false,
        gix::index::decode::Options::default(),
    )
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

fn write_to_workdir(
    file_path: &Path,
    blob: &[u8],
    mode: EntryMode,
    use_symlinks: bool,
) -> Result<(), std::io::Error> {
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if mode.is_link() && use_symlinks {
        remove_if_exists(file_path)?;
        create_symlink(blob, file_path)?;
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let want_exec = mode.kind() == EntryKind::BlobExecutable;
            let mode_ok = file_path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_file() && is_executable(&m) == want_exec);
            if mode_ok {
                fs::write(file_path, blob)?;
            } else {
                // Recreate with a broad mode so the kernel applies the umask.
                remove_if_exists(file_path)?;
                let create_mode = if want_exec { 0o777 } else { 0o666 };
                fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(create_mode)
                    .open(file_path)?
                    .write_all(blob)?;
            }
        }
        #[cfg(not(unix))]
        {
            // Unlink an existing symlink first; write() would follow it and corrupt the target.
            if file_path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                remove_if_exists(file_path)?;
            }
            fs::write(file_path, blob)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(blob: &[u8], link: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(blob), link)
}

#[cfg(windows)]
fn create_symlink(blob: &[u8], link: &Path) -> std::io::Result<()> {
    let target = std::str::from_utf8(blob)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Resolve target relative to symlink's parent to determine whether it's a directory.
    // If the target doesn't exist yet, default to symlink_file - matches git's own behavior.
    let is_dir = link
        .parent()
        .map_or_else(|| Path::new(target).to_path_buf(), |p| p.join(target))
        .is_dir();
    if is_dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

fn remove_if_exists(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(windows)]
    {
        // On Windows, remove_file fails on directory symlinks - use remove_dir for those.
        match path.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
            Ok(_) if fs::metadata(path).is_ok_and(|m| m.is_dir()) => {
                return fs::remove_dir(path);
            }
            Ok(_) => {}
        }
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
